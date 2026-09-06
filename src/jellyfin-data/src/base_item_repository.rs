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
    sea_query::{Alias, Expr, Order, Query},
};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    entities::{ancestor_id, base_item, item_value, linked_child, user_data},
    item_types::expand_item_type_aliases,
};

const HIERARCHY_ADVISORY_LOCK_KEY: i64 = 0x4241_5345_4954_454d;
const VIDEO_ITEM_TYPES_SQL: &str = "('Video', 'Movie', 'Episode', 'MusicVideo', 'Trailer', \
    'MediaBrowser.Controller.Entities.Video', \
    'MediaBrowser.Controller.Entities.Movies.Movie', \
    'MediaBrowser.Controller.Entities.TV.Episode', \
    'MediaBrowser.Controller.Entities.MusicVideo', \
    'MediaBrowser.Controller.Entities.Trailer')";
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

/// A deterministic item-by-name row discovered by a post-scan validator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewItemByNameEntity {
    pub id: Uuid,
    pub item_type: String,
    pub name: String,
    pub path: String,
    pub presentation_unique_key: String,
    pub date_created: DateTime<Utc>,
    pub date_modified: DateTime<Utc>,
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

/// One top series' official latest-TV grouping decision.
#[derive(Debug, Clone, PartialEq, Eq, FromQueryResult)]
pub struct LatestTvGroup {
    pub series_id: Uuid,
    pub most_recent_episode_id: Uuid,
    pub selected_container_id: Option<Uuid>,
    pub max_date: DateTime<Utc>,
    pub recent_child_count: i64,
}

#[derive(Debug, FromQueryResult)]
struct NearestAncestorId {
    item_id: Uuid,
    ancestor_id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct ItemIdRow {
    item_id: Uuid,
}

/// Minimal descendant fields needed while reconciling a library scan.
#[derive(Debug, Clone, PartialEq, Eq, FromQueryResult)]
pub struct DescendantScanCandidate {
    pub id: Uuid,
    pub item_type: String,
    pub path: Option<String>,
    pub parent_id: Option<Uuid>,
    pub season_id: Option<Uuid>,
}

/// Minimal fields needed to schedule metadata and image maintenance refreshes.
#[derive(Debug, Clone, PartialEq, Eq, FromQueryResult)]
pub struct MetadataRefreshCandidate {
    pub id: Uuid,
    pub item_type: String,
    pub series_id: Option<Uuid>,
}

/// Minimal fields needed to reconcile series, season, and episode hierarchy.
#[derive(Debug, Clone, PartialEq, Eq, FromQueryResult)]
pub struct TvHierarchyCandidate {
    pub id: Uuid,
    pub row_version: i64,
    pub item_type: String,
    pub path: Option<String>,
    pub parent_id: Option<Uuid>,
    pub index_number: Option<i32>,
    pub parent_index_number: Option<i32>,
    pub series_id: Option<Uuid>,
    pub season_id: Option<Uuid>,
    pub series_presentation_unique_key: Option<String>,
}

#[derive(Debug, Clone, Copy, FromQueryResult)]
struct BaseItemRowVersion {
    row_version: i64,
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
    AiredEpisodeOrderAscending,
    AiredEpisodeOrderDescending,
    AlbumAscending,
    AlbumDescending,
    AlbumArtistAscending,
    AlbumArtistDescending,
    ArtistAscending,
    ArtistDescending,
    OfficialRatingAscending,
    OfficialRatingDescending,
    StartDateAscending,
    StartDateDescending,
    IsFolderAscending,
    IsFolderDescending,
    IsUnplayedAscending,
    IsUnplayedDescending,
    IsPlayedAscending,
    IsPlayedDescending,
    SeriesSortNameAscending,
    SeriesSortNameDescending,
    VideoBitRateAscending,
    VideoBitRateDescending,
    AirTimeAscending,
    AirTimeDescending,
    StudioAscending,
    StudioDescending,
    IsFavoriteOrLikedAscending,
    IsFavoriteOrLikedDescending,
    DateLastContentAddedAscending,
    DateLastContentAddedDescending,
    ParentIndexNumberAscending,
    ParentIndexNumberDescending,
    IndexNumberAscending,
    IndexNumberDescending,
}

impl BaseItemOrder {
    #[must_use]
    pub const fn descending(self) -> Self {
        match self {
            Self::SortName => Self::SortNameDescending,
            Self::DateCreatedAscending => Self::DateCreatedDescending,
            Self::DatePlayedAscending => Self::DatePlayedDescending,
            Self::PremiereDateAscending => Self::PremiereDateDescending,
            Self::PlayCountAscending => Self::PlayCountDescending,
            Self::CommunityRatingAscending => Self::CommunityRatingDescending,
            Self::CriticRatingAscending => Self::CriticRatingDescending,
            Self::RuntimeTicksAscending => Self::RuntimeTicksDescending,
            Self::AiredEpisodeOrderAscending => Self::AiredEpisodeOrderDescending,
            Self::AlbumAscending => Self::AlbumDescending,
            Self::AlbumArtistAscending => Self::AlbumArtistDescending,
            Self::ArtistAscending => Self::ArtistDescending,
            Self::OfficialRatingAscending => Self::OfficialRatingDescending,
            Self::StartDateAscending => Self::StartDateDescending,
            Self::IsFolderAscending => Self::IsFolderDescending,
            Self::IsUnplayedAscending => Self::IsUnplayedDescending,
            Self::IsPlayedAscending => Self::IsPlayedDescending,
            Self::SeriesSortNameAscending => Self::SeriesSortNameDescending,
            Self::VideoBitRateAscending => Self::VideoBitRateDescending,
            Self::AirTimeAscending => Self::AirTimeDescending,
            Self::StudioAscending => Self::StudioDescending,
            Self::IsFavoriteOrLikedAscending => Self::IsFavoriteOrLikedDescending,
            Self::DateLastContentAddedAscending => Self::DateLastContentAddedDescending,
            Self::ParentIndexNumberAscending => Self::ParentIndexNumberDescending,
            Self::IndexNumberAscending => Self::IndexNumberDescending,
            other => other,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct BaseItemQuery {
    pub ids: Vec<Uuid>,
    pub exclude_ids: Vec<Uuid>,
    pub genres: Vec<String>,
    pub studios: Vec<String>,
    pub artists: Vec<String>,
    pub albums: Vec<String>,
    pub years: Vec<i32>,
    pub tags: Vec<String>,
    pub person: Option<String>,
    pub min_community_rating: Option<f64>,
    pub is_favorite: Option<bool>,
    pub is_folder: Option<bool>,
    pub is_liked: Option<bool>,
    pub is_favorite_or_liked: Option<bool>,
    pub parent_id: Option<Uuid>,
    /// Limits results to the union of these physical library parents.
    ///
    /// Jellyfin's grouped `UserView` exposes multiple collection folders as a
    /// single logical parent. Keeping that union in the database query avoids
    /// both cross-library leakage and application-side filtering.
    pub parent_ids: Vec<Uuid>,
    pub recursive: bool,
    pub search_term: Option<String>,
    pub include_item_types: Vec<String>,
    pub exclude_item_types: Vec<String>,
    pub media_types: Vec<String>,
    pub image_types: Vec<i16>,
    pub is_movie: Option<bool>,
    pub is_series: Option<bool>,
    pub is_news: Option<bool>,
    pub is_kids: Option<bool>,
    pub is_sports: Option<bool>,
    pub is_airing: Option<bool>,
    pub is_virtual_item: Option<bool>,
    pub group_versions_by_presentation_key: bool,
    /// Includes alternate-version rows that ordinary item pages fold into their primary.
    /// This is reserved for exact-id detail lookups after normal user-policy filters apply.
    pub include_alternate_versions: bool,
    pub user_id: Option<Uuid>,
    pub is_resumable: Option<bool>,
    pub is_played: Option<bool>,
    pub min_premiere_date: Option<DateTime<Utc>>,
    pub max_premiere_date: Option<DateTime<Utc>>,
    pub min_date_last_saved: Option<DateTime<Utc>>,
    pub min_date_last_saved_for_user: Option<DateTime<Utc>>,
    pub min_critic_rating: Option<f64>,
    pub has_overview: Option<bool>,
    pub has_official_rating: Option<bool>,
    pub has_parental_rating: Option<bool>,
    pub has_imdb_id: Option<bool>,
    pub has_tmdb_id: Option<bool>,
    pub has_tvdb_id: Option<bool>,
    pub has_subtitles: Option<bool>,
    pub has_theme_song: Option<bool>,
    pub has_theme_video: Option<bool>,
    pub has_special_feature: Option<bool>,
    pub has_trailer: Option<bool>,
    pub is_hd: Option<bool>,
    pub is_4k: Option<bool>,
    pub min_width: Option<i32>,
    pub max_width: Option<i32>,
    pub min_height: Option<i32>,
    pub max_height: Option<i32>,
    pub is_3d: Option<bool>,
    pub is_locked: Option<bool>,
    pub is_placeholder: Option<bool>,
    pub is_missing: Option<bool>,
    pub is_unaired: Option<bool>,
    pub index_number: Option<i32>,
    pub parent_index_number: Option<i32>,
    pub adjacent_to: Option<Uuid>,
    pub location_types: Vec<String>,
    pub exclude_location_types: Vec<String>,
    pub video_types: Vec<String>,
    pub series_statuses: Vec<String>,
    pub official_ratings: Vec<String>,
    pub audio_languages: Vec<String>,
    pub subtitle_languages: Vec<String>,
    pub person_ids: Vec<Uuid>,
    pub person_types: Vec<String>,
    pub studio_ids: Vec<Uuid>,
    pub genre_ids: Vec<Uuid>,
    pub artist_ids: Vec<Uuid>,
    pub exclude_artist_ids: Vec<Uuid>,
    pub album_artist_ids: Vec<Uuid>,
    pub contributing_artist_ids: Vec<Uuid>,
    pub album_ids: Vec<Uuid>,
    pub name_starts_with_or_greater: Option<String>,
    pub name_starts_with: Option<String>,
    pub name_less_than: Option<String>,
    pub collapse_box_set_items: bool,
    pub allowed_official_ratings: Vec<String>,
    pub allowed_parental_ratings: Vec<String>,
    pub block_unrated_items: Vec<String>,
    pub blocked_tags: Vec<String>,
    pub allowed_tags: Vec<String>,
    pub enabled_folders: Vec<Uuid>,
    pub enable_all_folders: bool,
    pub blocked_media_folders: Option<Vec<Uuid>>,
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

#[derive(Debug, Clone, PartialEq)]
pub struct ScoredBaseItem {
    pub item: base_item::Model,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScoredBaseItemPage {
    pub items: Vec<ScoredBaseItem>,
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

#[derive(Debug, Clone, Copy, FromQueryResult)]
struct ParentChildCount {
    parent_id: Uuid,
    child_count: i64,
}

#[derive(Debug, Clone, Copy, FromQueryResult)]
struct MediaSourceCount {
    item_id: Uuid,
    source_count: i64,
}

#[derive(Debug, Clone, Copy, FromQueryResult)]
struct ResumePageId {
    total_record_count: Option<i64>,
    id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductionYearPage {
    pub years: Vec<i32>,
    pub total_record_count: u64,
    pub start_index: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaStreamLanguageLists {
    pub audio: Vec<String>,
    pub subtitles: Vec<String>,
}

#[derive(Debug, FromQueryResult)]
struct MediaStreamLanguageRow {
    stream_type: i16,
    language: String,
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
    database: crate::SharedDatabase,
}

impl BaseItemRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
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

    /// Inserts a deterministic item or returns the row created by a concurrent caller.
    ///
    /// The existence check runs after taking the hierarchy advisory lock. This closes the race
    /// between sibling scan workers that discover the same series or season at the same time,
    /// while leaving [`Self::create`] strict for callers that expect duplicate identifiers to fail.
    ///
    /// # Errors
    ///
    /// Returns a validation, hierarchy, or database error.
    pub async fn create_if_absent(
        &self,
        item: NewBaseItem,
    ) -> Result<(base_item::Model, bool), BaseItemError> {
        validate_item_type(&item.item_type)?;
        let transaction = self.database.begin().await?;
        acquire_hierarchy_lock(&transaction).await?;
        if let Some(existing) = base_item::Entity::find_by_id(item.id)
            .one(&transaction)
            .await?
        {
            transaction.commit().await?;
            return Ok((existing, false));
        }
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
        Ok((inserted, true))
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

    /// Inserts every missing deterministic item-by-name entity in one statement.
    ///
    /// An existing canonical or CLR-qualified row with the same normalized
    /// name wins. Existing rows are never updated, preserving their metadata,
    /// images, user data, and stable client-facing identities. Deterministic
    /// identifiers plus `ON CONFLICT` make concurrent validator runs harmless.
    ///
    /// # Errors
    ///
    /// Returns a validation or database error.
    pub async fn create_missing_item_by_name_entities(
        &self,
        entities: &[NewItemByNameEntity],
    ) -> Result<u64, BaseItemError> {
        if entities.is_empty() {
            return Ok(0);
        }
        for entity in entities {
            validate_item_type(&entity.item_type)?;
        }

        let mut values = Vec::with_capacity(entities.len().saturating_mul(7));
        let mut sql = String::from(
            "WITH candidate(\
                 id, item_type, name, path, presentation_unique_key, date_created, date_modified\
             ) AS (VALUES ",
        );
        for (index, entity) in entities.iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            sql.push('(');
            push_bind(&mut sql, &mut values, entity.id, "");
            push_bind(&mut sql, &mut values, entity.item_type.clone(), ", ");
            push_bind(&mut sql, &mut values, entity.name.clone(), ", ");
            push_bind(&mut sql, &mut values, entity.path.clone(), ", ");
            push_bind(
                &mut sql,
                &mut values,
                entity.presentation_unique_key.clone(),
                ", ",
            );
            push_bind(&mut sql, &mut values, entity.date_created, ", ");
            push_bind(&mut sql, &mut values, entity.date_modified, ", ");
            sql.push(')');
        }
        sql.push_str(
            ") \
             INSERT INTO jellyfin.base_items \
                 (id, item_type, name, sort_name, path, is_folder, presentation_unique_key, \
                  date_created, date_modified) \
             SELECT candidate.id, candidate.item_type, candidate.name, candidate.name, \
                    candidate.path, \
                    CASE WHEN candidate.item_type IN ('MusicArtist', 'Person') \
                         THEN false ELSE true END, \
                    candidate.presentation_unique_key, \
                    candidate.date_created, candidate.date_modified \
             FROM candidate \
             WHERE NOT EXISTS (\
                 SELECT 1 FROM jellyfin.base_items AS existing \
                 WHERE existing.clean_name = jellyfin.normalize_search_text(candidate.name) \
                   AND (existing.item_type = candidate.item_type \
                        OR existing.item_type = CASE candidate.item_type \
                            WHEN 'Genre' THEN 'MediaBrowser.Controller.Entities.Genre' \
                            WHEN 'MusicGenre' THEN \
                                'MediaBrowser.Controller.Entities.Audio.MusicGenre' \
                            ELSE candidate.item_type END)\
             ) \
             ON CONFLICT (id) DO NOTHING",
        );
        let result = self
            .database
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .await?;
        Ok(result.rows_affected())
    }

    /// Loads an item by its stable identifier.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn get(&self, id: Uuid) -> Result<Option<base_item::Model>, BaseItemError> {
        Ok(base_item::Entity::find_by_id(id)
            .one(self.database.as_ref())
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
            .all(self.database.as_ref())
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
            .one(self.database.as_ref())
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
                       AND item_type IN {VIDEO_ITEM_TYPES_SQL}\
                 ), target_version AS (\
                     SELECT item.* \
                     FROM jellyfin.base_items AS item \
                     INNER JOIN requested \
                       ON COALESCE(item.primary_version_id, item.id) = requested.group_id \
                     WHERE item.id = $2 \
                       AND item.item_type IN {VIDEO_ITEM_TYPES_SQL}\
                 ) \
                 SELECT {BASE_ITEM_COLUMNS} FROM target_version"
            ),
            [source_item_id.into(), version_item_id.into()],
        );
        Ok(base_item::Model::find_by_statement(statement)
            .one(self.database.as_ref())
            .await?)
    }

    /// Loads every local video source in the version group containing `item_id`.
    ///
    /// The explicitly requested version is returned first, followed by the
    /// primary and remaining alternates in stable identifier order. Keeping the
    /// group expansion in PostgreSQL lets playback callers batch stream and
    /// attachment projection without issuing one lookup per version.
    ///
    /// # Errors
    ///
    /// Returns a database error when the group cannot be loaded.
    pub async fn media_source_versions(
        &self,
        item_id: Uuid,
    ) -> Result<Vec<base_item::Model>, BaseItemError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "WITH requested AS MATERIALIZED (\
                     SELECT COALESCE(primary_version_id, id) AS group_id \
                     FROM jellyfin.base_items \
                     WHERE id = $1 \
                       AND item_type IN {VIDEO_ITEM_TYPES_SQL}\
                 ) \
                 SELECT {BASE_ITEM_COLUMNS} \
                 FROM jellyfin.base_items AS item \
                 INNER JOIN requested \
                   ON COALESCE(item.primary_version_id, item.id) = requested.group_id \
                 WHERE item.item_type IN {VIDEO_ITEM_TYPES_SQL} \
                 ORDER BY CASE WHEN item.id = $1 THEN 0 ELSE 1 END, \
                          CASE WHEN item.primary_version_id IS NULL THEN 0 ELSE 1 END, \
                          item.id"
            ),
            [item_id.into()],
        );
        Ok(base_item::Model::find_by_statement(statement)
            .all(self.database.as_ref())
            .await?)
    }

    /// Loads every local video source belonging to any requested version group.
    ///
    /// Each source is returned once even when multiple requested identifiers name the same group.
    /// The primary source sorts before its alternates inside each group so page projection can
    /// preserve the official default-source ordering without issuing one query per item.
    ///
    /// # Errors
    ///
    /// Returns a database error when the groups cannot be loaded.
    pub async fn media_source_versions_for_items(
        &self,
        item_ids: &[Uuid],
    ) -> Result<Vec<base_item::Model>, BaseItemError> {
        if item_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut sql = format!(
            "WITH requested AS MATERIALIZED (\
                 SELECT DISTINCT COALESCE(item.primary_version_id, item.id) AS group_id \
                 FROM jellyfin.base_items AS item \
                 WHERE item.item_type IN {VIDEO_ITEM_TYPES_SQL}",
        );
        let mut values = Vec::with_capacity(item_ids.len());
        append_uuid_list_filter(&mut sql, &mut values, "item.id", item_ids);
        let _ = write!(
            sql,
            ") SELECT {BASE_ITEM_COLUMNS} \
             FROM jellyfin.base_items AS item \
             INNER JOIN requested \
               ON COALESCE(item.primary_version_id, item.id) = requested.group_id \
             WHERE item.item_type IN {VIDEO_ITEM_TYPES_SQL} \
             ORDER BY requested.group_id, \
                      CASE WHEN item.primary_version_id IS NULL THEN 0 ELSE 1 END, \
                      item.id"
        );
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

    /// Returns the requested item identifiers that satisfy a user's library access policy.
    ///
    /// This deliberately selects identifiers only. Callers that already loaded alternate media
    /// source rows can therefore discard hidden versions before loading their streams and
    /// attachments without fetching the same base-item rows a second time.
    ///
    /// # Errors
    ///
    /// Returns a database error when the visibility query fails.
    pub async fn visible_item_ids(
        &self,
        item_ids: &[Uuid],
        access_policy: &BaseItemQuery,
    ) -> Result<HashSet<Uuid>, BaseItemError> {
        if item_ids.is_empty() {
            return Ok(HashSet::new());
        }

        let mut select = base_item::Entity::find()
            .select_only()
            .column(base_item::Column::Id)
            .filter(base_item::Column::Id.is_in(item_ids.iter().copied()));
        if let Some(condition) = policy_filter_sql("\"base_items\"", access_policy) {
            select = select.filter(Expr::cust(condition));
        }
        Ok(select
            .into_tuple::<Uuid>()
            .all(self.database.as_ref())
            .await?
            .into_iter()
            .collect())
    }

    /// Counts the local media sources in each requested video's alternate-version group.
    ///
    /// The aggregate is set-based so DTO pages requesting `MediaSourceCount` never issue one
    /// version lookup per video. Missing and non-video identifiers are omitted.
    ///
    /// # Errors
    ///
    /// Returns a database error when the aggregate query fails.
    pub async fn media_source_counts(
        &self,
        item_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, u64>, BaseItemError> {
        self.media_source_counts_with_policy(item_ids, None).await
    }

    /// Counts the user-visible local media sources in each requested video's version group.
    ///
    /// The explicitly displayed version is always counted, while every sibling must satisfy the
    /// supplied standalone-item access policy. This matches Jellyfin's nullable count projection
    /// without materializing complete media-source rows.
    ///
    /// # Errors
    ///
    /// Returns a database error when the aggregate query fails.
    pub async fn visible_media_source_counts(
        &self,
        item_ids: &[Uuid],
        access_policy: &BaseItemQuery,
    ) -> Result<HashMap<Uuid, u64>, BaseItemError> {
        self.media_source_counts_with_policy(item_ids, Some(access_policy))
            .await
    }

    async fn media_source_counts_with_policy(
        &self,
        item_ids: &[Uuid],
        access_policy: Option<&BaseItemQuery>,
    ) -> Result<HashMap<Uuid, u64>, BaseItemError> {
        if item_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut sql = format!(
            "SELECT requested.id AS item_id, COUNT(version.id)::bigint AS source_count \
                       FROM jellyfin.base_items AS requested \
                       INNER JOIN jellyfin.base_items AS version \
                         ON COALESCE(version.primary_version_id, version.id) = \
                            COALESCE(requested.primary_version_id, requested.id) \
                        AND version.item_type IN {VIDEO_ITEM_TYPES_SQL} \
                       WHERE requested.item_type IN {VIDEO_ITEM_TYPES_SQL}"
        );
        if let Some(condition) =
            access_policy.and_then(|policy| policy_filter_sql("version", policy))
        {
            let _ = write!(sql, " AND (version.id = requested.id OR ({condition}))");
        }
        let mut values = Vec::with_capacity(item_ids.len());
        append_uuid_list_filter(&mut sql, &mut values, "requested.id", item_ids);
        sql.push_str(" GROUP BY requested.id");
        let rows = MediaSourceCount::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .all(self.database.as_ref())
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    row.item_id,
                    u64::try_from(row.source_count).unwrap_or_default(),
                )
            })
            .collect())
    }

    /// Returns each source id attached through a user-created alternate-version
    /// relationship together with its grouping parent id.
    ///
    /// The lookup is set-based so DTO pages can project `MediaSource.Type`
    /// without querying one relationship list per displayed video.
    ///
    /// # Errors
    ///
    /// Returns a database error when the relationship query fails.
    pub async fn linked_alternate_version_parents(
        &self,
        item_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Uuid>, BaseItemError> {
        if item_ids.is_empty() {
            return Ok(HashMap::new());
        }
        Ok(linked_child::Entity::find()
            .select_only()
            .column(linked_child::Column::ChildId)
            .column(linked_child::Column::ParentId)
            .filter(
                linked_child::Column::ChildType
                    .eq(crate::LinkedChildType::LinkedAlternateVersion as i16),
            )
            .filter(linked_child::Column::ChildId.is_in(item_ids.iter().copied()))
            .into_tuple::<(Uuid, Uuid)>()
            .all(self.database.as_ref())
            .await?
            .into_iter()
            .collect())
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
            .all(self.database.as_ref())
            .await?)
    }

    /// Detaches only the user-linked alternate sources in the version group
    /// containing `item_id`.
    ///
    /// Local alternate relationships created by scanning remain intact. Their
    /// persisted parent ids are used to reconstruct each local version group
    /// after the enclosing user-created group is split. One data-modifying CTE
    /// keeps that transition atomic and preserves every media row.
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
                 ), group_members AS MATERIALIZED (\
                     SELECT item.id \
                     FROM jellyfin.base_items AS item \
                     INNER JOIN requested \
                       ON item.id = requested.group_id \
                       OR item.primary_version_id = requested.group_id\
                 ), manual_links AS MATERIALIZED (\
                     SELECT link.child_id \
                     FROM jellyfin.linked_children AS link \
                     INNER JOIN requested ON requested.group_id = link.parent_id \
                     WHERE link.child_type = 3\
                 ), restored AS (\
                     UPDATE jellyfin.base_items AS item \
                     SET primary_version_id = (\
                         SELECT local.parent_id \
                         FROM jellyfin.linked_children AS local \
                         WHERE local.child_id = item.id AND local.child_type = 2 \
                         ORDER BY local.parent_id LIMIT 1\
                     ) \
                     WHERE EXISTS (SELECT 1 FROM manual_links) \
                       AND item.id IN (SELECT id FROM group_members) \
                       AND (item.id IN (SELECT child_id FROM manual_links) \
                            OR EXISTS (\
                                SELECT 1 FROM jellyfin.linked_children AS local \
                                WHERE local.child_id = item.id AND local.child_type = 2\
                            )) \
                     RETURNING item.id\
                 ), removed_links AS (\
                     DELETE FROM jellyfin.linked_children AS link \
                     USING requested \
                     WHERE link.parent_id = requested.group_id \
                       AND link.child_type = 3 \
                       AND (SELECT COUNT(*) FROM restored) >= 0 \
                     RETURNING link.child_id\
                 ) \
                 SELECT EXISTS (SELECT 1 FROM requested) AS found, \
                        COUNT(*) AS cleared_count \
                 FROM removed_links",
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

    /// Merges existing video groups through user-created alternate links.
    ///
    /// Existing untyped groups are conservatively recorded as local groups
    /// before the merge. The selected groups are then flattened for the shared
    /// playback queries while `LocalAlternateVersion` links retain enough
    /// information for a later split to reconstruct every scan-created group.
    ///
    /// # Errors
    ///
    /// Returns `InvalidItemType` when fewer than two existing rows are supplied,
    /// or a database error when the transaction cannot be completed.
    pub async fn merge_linked_alternate_versions(
        &self,
        item_ids: &[Uuid],
    ) -> Result<Uuid, BaseItemError> {
        if item_ids.len() < 2 {
            return Err(BaseItemError::InvalidItemType);
        }

        let (sql, values) = linked_alternate_version_merge_sql(item_ids);

        let transaction = self.database.begin().await?;
        let result = transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .await?
            .ok_or_else(|| {
                DbErr::RecordNotFound("linked version merge returned no row".to_owned())
            })?;
        if result.try_get::<i64>("", "requested_count")? < 2 {
            return Err(BaseItemError::InvalidItemType);
        }
        let primary_id = result.try_get::<Uuid>("", "primary_id")?;
        transaction.commit().await?;
        Ok(primary_id)
    }

    /// Merges scan-discovered video identifiers into one local version group.
    ///
    /// `PostgreSQL` expands every requested row to its current version group,
    /// chooses a stable primary identifier, rewrites all members, and records
    /// each relationship as `LocalAlternateVersion` in one data-modifying CTE.
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
             ), removed_local_links AS (\
                 DELETE FROM jellyfin.linked_children AS link \
                 WHERE link.child_type = 2 \
                   AND link.child_id IN (SELECT id FROM merge_members) \
                 RETURNING link.child_id\
             ), local_links AS (\
                 INSERT INTO jellyfin.linked_children \
                     (parent_id, child_id, child_type, sort_order) \
                 SELECT primary_version.id, member.id, 2, NULL \
                 FROM primary_version \
                 CROSS JOIN merge_members AS member \
                 WHERE member.id <> primary_version.id \
                   AND (SELECT COUNT(*) FROM removed_local_links) >= 0 \
                 ON CONFLICT (parent_id, child_id) DO NOTHING \
                 RETURNING child_id\
             ), updated AS (\
                 UPDATE jellyfin.base_items AS item \
                 SET primary_version_id = CASE \
                         WHEN item.id = (SELECT id FROM primary_version) THEN NULL \
                         ELSE (SELECT id FROM primary_version) \
                     END \
                 FROM merge_members \
                 WHERE item.id = merge_members.id \
                   AND item.primary_version_id IS DISTINCT FROM CASE \
                         WHEN item.id = (SELECT id FROM primary_version) THEN NULL \
                         ELSE (SELECT id FROM primary_version) \
                       END \
                   AND (SELECT COUNT(*) FROM local_links) >= 0 \
                 RETURNING item.id\
             ) \
             SELECT (SELECT COUNT(*) FROM requested_items) AS requested_count, \
                    (SELECT id FROM primary_version) AS primary_id, \
                    (SELECT COUNT(*) FROM updated) AS updated_count",
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

    /// Applies a batch of scan-discovered local alternate assignments.
    ///
    /// The base-item back references and typed linked-child rows change in one
    /// transaction. Reassigning a child removes only its previous local link;
    /// user-created alternate and ordinary manual links are left untouched.
    ///
    /// # Errors
    ///
    /// Returns a database error when the batch cannot be applied.
    pub async fn assign_local_alternate_versions(
        &self,
        assignments: &[(Uuid, Uuid)],
    ) -> Result<Vec<Uuid>, BaseItemError> {
        let mut unique = HashMap::with_capacity(assignments.len());
        for (child_id, parent_id) in assignments.iter().copied() {
            if child_id != parent_id {
                unique.entry(child_id).or_insert(parent_id);
            }
        }
        if unique.is_empty() {
            return Ok(Vec::new());
        }

        let mut values = Vec::with_capacity(unique.len().saturating_mul(2));
        let mut sql = String::from("WITH assignments(child_id, parent_id) AS (VALUES ");
        for (index, (child_id, parent_id)) in unique.into_iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            values.push(child_id.into());
            let child_bind = values.len();
            values.push(parent_id.into());
            let parent_bind = values.len();
            let _ = write!(sql, "(${child_bind}::uuid, ${parent_bind}::uuid)");
        }
        sql.push_str(
            "), valid AS MATERIALIZED (\
                 SELECT assignment.child_id, assignment.parent_id \
                 FROM assignments AS assignment \
                 INNER JOIN jellyfin.base_items AS child ON child.id = assignment.child_id \
                 INNER JOIN jellyfin.base_items AS parent ON parent.id = assignment.parent_id\
             ), removed_local_links AS (\
                 DELETE FROM jellyfin.linked_children AS link \
                 USING valid \
                 WHERE link.child_id = valid.child_id \
                   AND link.child_type = 2 \
                   AND link.parent_id <> valid.parent_id \
                 RETURNING link.child_id\
             ), local_links AS (\
                 INSERT INTO jellyfin.linked_children \
                     (parent_id, child_id, child_type, sort_order) \
                 SELECT parent_id, child_id, 2, NULL FROM valid \
                 WHERE (SELECT COUNT(*) FROM removed_local_links) >= 0 \
                 ON CONFLICT (parent_id, child_id) DO NOTHING \
                 RETURNING child_id\
             ), updated AS (\
                 UPDATE jellyfin.base_items AS item \
                 SET primary_version_id = valid.parent_id \
                 FROM valid \
                 WHERE item.id = valid.child_id \
                   AND item.primary_version_id IS DISTINCT FROM valid.parent_id \
                   AND (SELECT COUNT(*) FROM local_links) >= 0 \
                 RETURNING item.id\
             ) \
             SELECT id FROM updated",
        );
        let transaction = self.database.begin().await?;
        let changed = transaction
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .await?
            .into_iter()
            .map(|row| row.try_get::<Uuid>("", "id"))
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit().await?;
        Ok(changed)
    }

    /// Uses the `PostgreSQL` partial hash index to test an exact item path.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn exists_by_path(&self, path: &str) -> Result<bool, BaseItemError> {
        Ok(base_item::Entity::find()
            .filter(base_item::Column::Path.eq(path))
            .one(self.database.as_ref())
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
            .one(self.database.as_ref())
            .await?)
    }

    /// Returns true when at least one persisted item advertises the production year.
    ///
    /// This intentionally uses a selective `LIMIT 1` lookup so `PostgreSQL` can
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
            .one(self.database.as_ref())
            .await?
            .is_some())
    }

    /// Queries distinct positive production years through `PostgreSQL`.
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

    /// Queries distinct non-empty official ratings through `PostgreSQL`.
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

    /// Returns distinct audio and subtitle languages for filtered items.
    ///
    /// Owned rows, including alternate media versions, intentionally remain in
    /// scope because an alternate can be the only source carrying a language.
    /// Null and empty persisted values use Jellyfin's `und` language code.
    ///
    /// # Errors
    ///
    /// Returns a database error when the set-based language query fails.
    pub async fn media_stream_languages_including_owned(
        &self,
        query: &BaseItemQuery,
    ) -> Result<MediaStreamLanguageLists, BaseItemError> {
        // Select stream-bearing rows with the requested item scope, but evaluate
        // access against the displayed owner. Alternate rows commonly omit
        // ratings and tags that live on their primary, so applying policy to the
        // alternate itself would silently drop its languages.
        let mut candidate_query = query.clone();
        candidate_query.allowed_official_ratings.clear();
        candidate_query.allowed_parental_ratings.clear();
        candidate_query.block_unrated_items.clear();
        candidate_query.blocked_tags.clear();
        candidate_query.allowed_tags.clear();
        candidate_query.enabled_folders.clear();
        candidate_query.enable_all_folders = true;
        candidate_query.blocked_media_folders = None;

        let mut values = Vec::new();
        let mut sql = String::from(
            "WITH candidates AS (\
                 SELECT item.id, item.primary_version_id, item.data \
                 FROM jellyfin.base_items AS item \
                 WHERE item.item_type <> 'PLACEHOLDER'",
        );
        append_raw_item_filters(&mut sql, &mut values, &candidate_query, true);
        if let Some(condition) = policy_filter_sql("access_item", query) {
            sql.push_str(
                "), filtered AS (\
                     SELECT candidate.id \
                     FROM candidates AS candidate \
                     LEFT JOIN jellyfin.base_items AS owner \
                       ON candidate.data ->> 'OwnerId' = owner.id::text \
                     JOIN jellyfin.base_items AS access_item \
                       ON access_item.id = COALESCE(\
                           candidate.primary_version_id, owner.id, candidate.id\
                       ) \
                     WHERE (",
            );
            sql.push_str(&condition);
            sql.push(')');
        } else {
            sql.push_str("), filtered AS (SELECT candidate.id FROM candidates AS candidate");
        }
        sql.push_str(
            ") \
             SELECT stream.stream_type, \
                    COALESCE(NULLIF(stream.language, ''), 'und') AS language \
             FROM filtered \
             JOIN jellyfin.media_streams AS stream ON stream.item_id = filtered.id \
             WHERE stream.stream_type IN (0, 2) \
             GROUP BY stream.stream_type, \
                      COALESCE(NULLIF(stream.language, ''), 'und') \
             ORDER BY stream.stream_type, language",
        );

        let rows = MediaStreamLanguageRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .all(self.database.as_ref())
        .await?;
        let mut languages = MediaStreamLanguageLists::default();
        for row in rows {
            match row.stream_type {
                0 => languages.audio.push(row.language),
                2 => languages.subtitles.push(row.language),
                _ => unreachable!("language query only selects audio and subtitle streams"),
            }
        }
        Ok(languages)
    }

    /// Queries persisted library items with stable sorting and database-side
    /// count, offset, and limit.
    ///
    /// # Errors
    ///
    /// Returns a database error when hierarchy or item queries fail.
    #[allow(clippy::too_many_lines)]
    pub async fn query(&self, query: &BaseItemQuery) -> Result<BaseItemPage, BaseItemError> {
        if let Some(is_resumable) = query.is_resumable {
            let user_id = query.user_id.ok_or(BaseItemError::UserRequired)?;
            return if is_resumable {
                self.query_resumable(user_id, query).await
            } else {
                self.query_not_resumable(user_id, query).await
            };
        }
        let requires_user_id = matches!(
            query.order,
            BaseItemOrder::IsUnplayedAscending
                | BaseItemOrder::IsUnplayedDescending
                | BaseItemOrder::IsPlayedAscending
                | BaseItemOrder::IsPlayedDescending
                | BaseItemOrder::IsFavoriteOrLikedAscending
                | BaseItemOrder::IsFavoriteOrLikedDescending
        );
        if requires_user_id && query.user_id.is_none() {
            return Err(BaseItemError::UserRequired);
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
                | BaseItemOrder::IsUnplayedAscending
                | BaseItemOrder::IsUnplayedDescending
                | BaseItemOrder::IsPlayedAscending
                | BaseItemOrder::IsPlayedDescending
                | BaseItemOrder::IsFavoriteOrLikedAscending
                | BaseItemOrder::IsFavoriteOrLikedDescending
                | BaseItemOrder::AlbumArtistAscending
                | BaseItemOrder::AlbumArtistDescending
                | BaseItemOrder::ArtistAscending
                | BaseItemOrder::ArtistDescending
                | BaseItemOrder::StudioAscending
                | BaseItemOrder::StudioDescending
                | BaseItemOrder::VideoBitRateAscending
                | BaseItemOrder::VideoBitRateDescending
                | BaseItemOrder::AiredEpisodeOrderAscending
                | BaseItemOrder::AiredEpisodeOrderDescending
                | BaseItemOrder::AlbumAscending
                | BaseItemOrder::AlbumDescending
                | BaseItemOrder::OfficialRatingAscending
                | BaseItemOrder::OfficialRatingDescending
                | BaseItemOrder::StartDateAscending
                | BaseItemOrder::StartDateDescending
                | BaseItemOrder::IsFolderAscending
                | BaseItemOrder::IsFolderDescending
                | BaseItemOrder::SeriesSortNameAscending
                | BaseItemOrder::SeriesSortNameDescending
                | BaseItemOrder::AirTimeAscending
                | BaseItemOrder::AirTimeDescending
                | BaseItemOrder::DateLastContentAddedAscending
                | BaseItemOrder::DateLastContentAddedDescending
                | BaseItemOrder::ParentIndexNumberAscending
                | BaseItemOrder::ParentIndexNumberDescending
                | BaseItemOrder::IndexNumberAscending
                | BaseItemOrder::IndexNumberDescending
        ) {
            return self.query_by_extended_sort(query).await;
        }
        if query.group_versions_by_presentation_key {
            return self.query_grouped_versions(query).await;
        }
        if query_uses_advanced_filters(query) {
            return self.query_by_filtered_cte(query).await;
        }
        let mut select =
            base_item::Entity::find().filter(base_item::Column::ItemType.ne("PLACEHOLDER"));
        if !query.include_alternate_versions {
            select = select.filter(base_item::Column::PrimaryVersionId.is_null());
        }
        select = select.filter(Expr::cust(
            "(data ->> 'OwnerId') IS NULL OR (data ->> 'ExtraType') IS NOT NULL",
        ));
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
        if !query.parent_ids.is_empty() {
            if query.recursive {
                let descendants = Query::select()
                    .column(ancestor_id::Column::ItemId)
                    .from((Alias::new("jellyfin"), ancestor_id::Entity))
                    .and_where(
                        ancestor_id::Column::ParentItemId.is_in(query.parent_ids.iter().copied()),
                    )
                    .to_owned();
                select = select.filter(base_item::Column::Id.in_subquery(descendants));
            } else {
                select = select
                    .filter(base_item::Column::ParentId.is_in(query.parent_ids.iter().copied()));
            }
        }
        if let Some(search_term) = query
            .search_term
            .as_deref()
            .map(str::trim)
            .filter(|term| !term.is_empty())
        {
            let (clean_pattern, original_pattern) = search_term_patterns(search_term);
            select = select.filter(Expr::cust_with_values(
                "clean_name LIKE $1 OR ((data ->> 'OriginalTitle') IS NOT NULL \
                 AND (data ->> 'OriginalTitle') ILIKE $2)",
                [clean_pattern, original_pattern],
            ));
        }
        if !query.include_item_types.is_empty() {
            let item_types = expand_item_type_aliases(&query.include_item_types);
            select = select.filter(base_item::Column::ItemType.is_in(item_types));
        }
        if !query.exclude_item_types.is_empty() {
            let item_types = expand_item_type_aliases(&query.exclude_item_types);
            select = select.filter(base_item::Column::ItemType.is_not_in(item_types));
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
        if let Some(is_airing) = query.is_airing {
            select = select.filter(Expr::cust(airing_expression("", is_airing)));
        }
        if let Some(is_virtual_item) = query.is_virtual_item {
            select = select.filter(base_item::Column::IsVirtualItem.eq(is_virtual_item));
        }
        if let Some(is_played) = query.is_played {
            let user_id = query.user_id.ok_or(BaseItemError::UserRequired)?;
            let mut values = Vec::new();
            let mut condition = String::new();
            append_is_played_filter(
                &mut condition,
                &mut values,
                user_id,
                "\"base_items\"",
                is_played,
            );
            select = select.filter(Expr::cust_with_values(condition, values));
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
            select =
                select.filter(base_item::Column::ProductionYear.is_in(query.years.iter().copied()));
        }
        if let Some(person) = query
            .person
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            select = select.filter(person_exists_expression(person));
        }
        if let Some(min_community_rating) = query.min_community_rating {
            select = select.filter(community_rating_expression(min_community_rating));
        }
        if let Some(min_premiere_date) = query.min_premiere_date {
            select = select.filter(base_item::Column::PremiereDate.gte(min_premiere_date));
        }
        if let Some(condition) = policy_filter_sql("\"base_items\"", query) {
            select = select.filter(Expr::cust(condition));
        }
        let total_record_count = if total_count_enabled(query) {
            Some(select.clone().count(self.database.as_ref()).await?)
        } else {
            None
        };
        let mut select = match query.order {
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
        let items = select.all(self.database.as_ref()).await?;
        Ok(BaseItemPage {
            total_record_count: page_total_record_count(total_record_count, items.len()),
            items,
            start_index: query.start_index,
        })
    }

    /// Counts direct children for multiple parents in one filtered query.
    ///
    /// The supplied item and user-policy filters are applied before grouping,
    /// so callers can expose visible child counts without issuing one query per
    /// folder.
    ///
    /// # Errors
    ///
    /// Returns a database error when the aggregate query fails.
    pub async fn child_counts_by_parent(
        &self,
        query: &BaseItemQuery,
    ) -> Result<HashMap<Uuid, u64>, BaseItemError> {
        if query.parent_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut direct_query = query.clone();
        direct_query.parent_id = None;
        direct_query.recursive = false;
        let (cte, values) = filtered_query_cte(&direct_query);
        let rows = ParentChildCount::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "{cte} SELECT parent_id, COUNT(*)::bigint AS child_count \
                 FROM filtered WHERE parent_id IS NOT NULL GROUP BY parent_id"
            ),
            values,
        ))
        .all(self.database.as_ref())
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    row.parent_id,
                    u64::try_from(row.child_count).unwrap_or_default(),
                )
            })
            .collect())
    }

    /// Counts immediate children using Jellyfin's folder, season, linked-child, and merged-folder
    /// rules. All requested parents are aggregated in one PostgreSQL query.
    ///
    /// # Errors
    ///
    /// Returns a database error when the aggregate query fails.
    pub async fn dto_child_counts(
        &self,
        parent_ids: &[Uuid],
        include_virtual: bool,
    ) -> Result<HashMap<Uuid, u64>, BaseItemError> {
        if parent_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let values = parent_ids
            .iter()
            .copied()
            .map(SeaValue::from)
            .collect::<Vec<_>>();
        let requested_values = (1..=parent_ids.len())
            .map(|index| {
                let mut placeholder = String::from("($");
                write!(placeholder, "{index}::uuid)").expect("writing to a String cannot fail");
                placeholder
            })
            .collect::<Vec<_>>()
            .join(", ");
        let virtual_filter = if include_virtual {
            ""
        } else {
            " AND child.is_virtual_item = false"
        };
        let sql = format!(
            "WITH requested(parent_id) AS (VALUES {requested_values}), \
             hierarchical AS (\
                 SELECT child.parent_id, COUNT(*)::bigint AS child_count \
                 FROM jellyfin.base_items AS child \
                 INNER JOIN requested ON requested.parent_id = child.parent_id \
                 WHERE child.season_id IS NULL{virtual_filter} \
                 GROUP BY child.parent_id\
             ), season_children AS (\
                 SELECT child.season_id AS parent_id, COUNT(*)::bigint AS child_count \
                 FROM jellyfin.base_items AS child \
                 INNER JOIN requested ON requested.parent_id = child.season_id \
                 WHERE child.season_id IS NOT NULL{virtual_filter} \
                 GROUP BY child.season_id\
             ), linked AS (\
                 SELECT link.parent_id, COUNT(*)::bigint AS child_count \
                 FROM jellyfin.linked_children AS link \
                 INNER JOIN requested ON requested.parent_id = link.parent_id \
                 GROUP BY link.parent_id\
             ), group_members AS (\
                 SELECT requested.parent_id, member.id AS member_id, \
                        COUNT(*) OVER (PARTITION BY requested.parent_id) AS member_count \
                 FROM requested \
                 INNER JOIN jellyfin.base_items AS target ON target.id = requested.parent_id \
                 INNER JOIN jellyfin.base_items AS member \
                   ON member.is_folder = true \
                  AND target.presentation_unique_key IS NOT NULL \
                  AND member.presentation_unique_key = target.presentation_unique_key\
             ), merged_children AS (\
                 SELECT members.parent_id, \
                        COALESCE(child.presentation_unique_key, child.id::text) AS child_key \
                 FROM group_members AS members \
                 INNER JOIN jellyfin.base_items AS child \
                   ON (child.parent_id = members.member_id AND child.season_id IS NULL) \
                   OR child.season_id = members.member_id \
                 WHERE members.member_count > 1{virtual_filter}\
             ), merged AS (\
                 SELECT groups.parent_id, \
                        COUNT(DISTINCT children.child_key)::bigint AS child_count \
                 FROM (\
                     SELECT DISTINCT parent_id FROM group_members WHERE member_count > 1\
                 ) AS groups \
                 LEFT JOIN merged_children AS children USING (parent_id) \
                 GROUP BY groups.parent_id\
             ) \
             SELECT requested.parent_id, \
                    COALESCE(\
                        merged.child_count, \
                        CASE WHEN COALESCE(linked.child_count, 0) > 0 \
                             THEN linked.child_count \
                             ELSE COALESCE(hierarchical.child_count, 0) \
                                + COALESCE(season_children.child_count, 0) \
                        END, \
                        0\
                    )::bigint AS child_count \
             FROM requested \
             LEFT JOIN hierarchical USING (parent_id) \
             LEFT JOIN season_children USING (parent_id) \
             LEFT JOIN linked USING (parent_id) \
             LEFT JOIN merged USING (parent_id)"
        );
        let rows = ParentChildCount::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .all(self.database.as_ref())
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    row.parent_id,
                    u64::try_from(row.child_count).unwrap_or_default(),
                )
            })
            .collect())
    }

    /// Counts visible, real leaf descendants for multiple folders in one query.
    ///
    /// Folder traversal follows both the physical hierarchy and linked children, expands merged
    /// folder rows that share a presentation key, and terminates linked-child cycles. The filtered
    /// leaf set reuses ordinary item-query access rules, including alternate-version, owned-item,
    /// virtual-item, library, tag, and parental restrictions.
    ///
    /// # Errors
    ///
    /// Returns a database error when the aggregate query fails.
    pub async fn dto_recursive_item_counts(
        &self,
        parent_ids: &[Uuid],
        query: &BaseItemQuery,
    ) -> Result<HashMap<Uuid, u64>, BaseItemError> {
        if parent_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let (cte, mut values) = filtered_query_cte(query);
        let mut sql = cte.replacen("WITH ", "WITH RECURSIVE ", 1);
        let requested_values = parent_ids
            .iter()
            .map(|parent_id| {
                values.push((*parent_id).into());
                let mut placeholder = String::from("($");
                write!(placeholder, "{}::uuid)", values.len())
                    .expect("writing to a String cannot fail");
                placeholder
            })
            .collect::<Vec<_>>()
            .join(", ");
        write!(
            sql,
            ", requested(parent_id) AS (VALUES {requested_values}), \
             group_members AS (\
                 SELECT requested.parent_id, member.id AS member_id \
                 FROM requested \
                 INNER JOIN jellyfin.base_items AS target ON target.id = requested.parent_id \
                 INNER JOIN jellyfin.base_items AS member \
                   ON member.is_folder = true \
                  AND (member.id = target.id \
                       OR (target.presentation_unique_key IS NOT NULL \
                           AND member.presentation_unique_key = target.presentation_unique_key))\
             ), traversal(parent_id, item_id) AS (\
                 SELECT parent_id, member_id FROM group_members \
                 UNION \
                 SELECT traversal.parent_id, edge.child_id \
                 FROM traversal \
                 CROSS JOIN LATERAL (\
                     SELECT child.id AS child_id \
                     FROM jellyfin.base_items AS child \
                     WHERE child.parent_id = traversal.item_id \
                     UNION \
                     SELECT link.child_id \
                     FROM jellyfin.linked_children AS link \
                     WHERE link.parent_id = traversal.item_id\
                 ) AS edge\
             ), counts AS (\
                 SELECT traversal.parent_id, COUNT(DISTINCT leaf.id)::bigint AS child_count \
                 FROM traversal \
                 INNER JOIN filtered AS leaf ON leaf.id = traversal.item_id \
                 WHERE leaf.is_folder = false \
                 GROUP BY traversal.parent_id\
             ) \
             SELECT requested.parent_id, COALESCE(counts.child_count, 0)::bigint AS child_count \
             FROM requested LEFT JOIN counts USING (parent_id)"
        )
        .expect("writing to a String cannot fail");

        let rows = ParentChildCount::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .all(self.database.as_ref())
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    row.parent_id,
                    u64::try_from(row.child_count).unwrap_or_default(),
                )
            })
            .collect())
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
        query: &BaseItemQuery,
    ) -> Result<BaseItemCounts, BaseItemError> {
        let (cte, values) = filtered_query_cte(query);
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                r"{cte}
            SELECT
                COUNT(*) FILTER (WHERE item.item_type IN (
                    'Movie', 'MediaBrowser.Controller.Entities.Movies.Movie'
                ))::bigint AS movie_count,
                COUNT(*) FILTER (WHERE item.item_type IN (
                    'Series', 'MediaBrowser.Controller.Entities.TV.Series'
                ))::bigint AS series_count,
                COUNT(*) FILTER (WHERE item.item_type IN (
                    'Episode', 'MediaBrowser.Controller.Entities.TV.Episode'
                ))::bigint AS episode_count,
                COUNT(*) FILTER (WHERE item.item_type IN (
                    'MusicArtist', 'MediaBrowser.Controller.Entities.Audio.MusicArtist'
                ))::bigint AS artist_count,
                COUNT(*) FILTER (WHERE item.item_type = 'Program')::bigint AS program_count,
                COUNT(*) FILTER (WHERE item.item_type IN (
                    'Trailer', 'MediaBrowser.Controller.Entities.Trailer'
                ))::bigint AS trailer_count,
                COUNT(*) FILTER (WHERE item.item_type IN (
                    'Audio', 'MediaBrowser.Controller.Entities.Audio.Audio'
                ))::bigint AS song_count,
                COUNT(*) FILTER (WHERE item.item_type IN (
                    'MusicAlbum', 'MediaBrowser.Controller.Entities.Audio.MusicAlbum'
                ))::bigint AS album_count,
                COUNT(*) FILTER (WHERE item.item_type IN (
                    'MusicVideo', 'MediaBrowser.Controller.Entities.MusicVideo'
                ))::bigint AS music_video_count,
                COUNT(*) FILTER (WHERE item.item_type IN (
                    'BoxSet', 'MediaBrowser.Controller.Entities.Movies.BoxSet'
                ))::bigint AS box_set_count,
                COUNT(*) FILTER (WHERE item.item_type IN (
                    'Book', 'MediaBrowser.Controller.Entities.Book'
                ))::bigint AS book_count,
                COUNT(*)::bigint AS item_count
            FROM filtered AS item
            "
            ),
            values,
        );
        base_item::Model::find_by_statement(statement)
            .into_model::<BaseItemCounts>()
            .one(self.database.as_ref())
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
            total_record_count: count.map_or_else(
                || u64::try_from(items.len()).unwrap_or(u64::MAX),
                |count| u64::try_from(count).unwrap_or_default(),
            ),
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
        self.query_resumable_page(cte, values, query).await
    }

    #[allow(clippy::too_many_lines)]
    async fn query_resumable_page(
        &self,
        cte: String,
        mut values: Vec<SeaValue>,
        query: &BaseItemQuery,
    ) -> Result<BaseItemPage, BaseItemError> {
        let transaction = self
            .database
            .begin_with_config(
                Some(IsolationLevel::RepeatableRead),
                Some(AccessMode::ReadOnly),
            )
            .await?;
        let mut sql = cte;
        if total_count_enabled(query) {
            sql.push_str(
                ", page_ids AS MATERIALIZED (\
                     SELECT id, ROW_NUMBER() OVER (\
                         ORDER BY resume_last_played_date DESC NULLS LAST, id\
                     ) AS page_order \
                     FROM filtered \
                     ORDER BY resume_last_played_date DESC NULLS LAST, id",
            );
            push_bind(
                &mut sql,
                &mut values,
                i64::try_from(query.start_index).unwrap_or(i64::MAX),
                " OFFSET ",
            );
            if let Some(limit) = query.limit {
                push_bind(
                    &mut sql,
                    &mut values,
                    i64::try_from(limit).unwrap_or(i64::MAX),
                    " LIMIT ",
                );
            }
            sql.push_str(
                ") \
                 SELECT total.total_record_count, page_ids.id \
                 FROM (SELECT COUNT(*)::bigint AS total_record_count FROM filtered) AS total \
                 LEFT JOIN page_ids ON true \
                 ORDER BY page_ids.page_order",
            );
        } else {
            sql.push_str(
                " SELECT NULL::bigint AS total_record_count, id \
                  FROM filtered \
                  ORDER BY resume_last_played_date DESC NULLS LAST, id",
            );
            push_bind(
                &mut sql,
                &mut values,
                i64::try_from(query.start_index).unwrap_or(i64::MAX),
                " OFFSET ",
            );
            if let Some(limit) = query.limit {
                push_bind(
                    &mut sql,
                    &mut values,
                    i64::try_from(limit).unwrap_or(i64::MAX),
                    " LIMIT ",
                );
            }
        }

        let page_rows = ResumePageId::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .all(&transaction)
        .await?;
        let total_record_count = page_rows
            .first()
            .and_then(|row| row.total_record_count)
            .map(|count| u64::try_from(count).unwrap_or_default());
        let page_ids = page_rows
            .into_iter()
            .filter_map(|row| row.id)
            .collect::<Vec<_>>();

        let page_items = if page_ids.is_empty() {
            Vec::new()
        } else {
            base_item::Entity::find()
                .filter(base_item::Column::Id.is_in(page_ids.clone()))
                .all(&transaction)
                .await?
        };
        let mut items_by_id = page_items
            .into_iter()
            .map(|item| (item.id, item))
            .collect::<HashMap<_, _>>();
        let items = page_ids
            .into_iter()
            .map(|id| {
                items_by_id.remove(&id).ok_or_else(|| {
                    BaseItemError::Database(DbErr::RecordNotFound(
                        "resume page item disappeared inside repeatable-read transaction"
                            .to_owned(),
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit().await?;

        Ok(BaseItemPage {
            total_record_count: total_record_count
                .unwrap_or_else(|| u64::try_from(items.len()).unwrap_or(u64::MAX)),
            items,
            start_index: query.start_index,
        })
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
            BaseItemOrder::PremiereDateDescending => "premiere_date DESC NULLS LAST, sort_name, id",
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
            BaseItemOrder::CriticRatingDescending => "critic_rating DESC NULLS LAST, sort_name, id",
            BaseItemOrder::RuntimeTicksAscending => "runtime_ticks ASC NULLS LAST, sort_name, id",
            BaseItemOrder::RuntimeTicksDescending => "runtime_ticks DESC NULLS LAST, sort_name, id",
            BaseItemOrder::AiredEpisodeOrderAscending => {
                "aired_episode_order ASC NULLS LAST, sort_name, id"
            }
            BaseItemOrder::AiredEpisodeOrderDescending => {
                "aired_episode_order DESC NULLS LAST, sort_name, id"
            }
            BaseItemOrder::AlbumAscending => "album ASC NULLS LAST, sort_name, id",
            BaseItemOrder::AlbumDescending => "album DESC NULLS LAST, sort_name, id",
            BaseItemOrder::AlbumArtistAscending => "album_artist ASC NULLS LAST, sort_name, id",
            BaseItemOrder::AlbumArtistDescending => "album_artist DESC NULLS LAST, sort_name, id",
            BaseItemOrder::ArtistAscending => "artist ASC NULLS LAST, sort_name, id",
            BaseItemOrder::ArtistDescending => "artist DESC NULLS LAST, sort_name, id",
            BaseItemOrder::OfficialRatingAscending => {
                "official_rating ASC NULLS LAST, sort_name, id"
            }
            BaseItemOrder::OfficialRatingDescending => {
                "official_rating DESC NULLS LAST, sort_name, id"
            }
            BaseItemOrder::StartDateAscending => "start_date ASC NULLS LAST, sort_name, id",
            BaseItemOrder::StartDateDescending => "start_date DESC NULLS LAST, sort_name, id",
            BaseItemOrder::IsFolderAscending => "is_folder ASC, sort_name, id",
            BaseItemOrder::IsFolderDescending => "is_folder DESC, sort_name, id",
            BaseItemOrder::IsUnplayedAscending => "is_unplayed ASC, sort_name, id",
            BaseItemOrder::IsUnplayedDescending => "is_unplayed DESC, sort_name, id",
            BaseItemOrder::IsPlayedAscending => "is_played ASC, sort_name, id",
            BaseItemOrder::IsPlayedDescending => "is_played DESC, sort_name, id",
            BaseItemOrder::SeriesSortNameAscending => {
                "series_sort_name ASC NULLS LAST, sort_name, id"
            }
            BaseItemOrder::SeriesSortNameDescending => {
                "series_sort_name DESC NULLS LAST, sort_name, id"
            }
            BaseItemOrder::VideoBitRateAscending => "video_bit_rate ASC NULLS LAST, sort_name, id",
            BaseItemOrder::VideoBitRateDescending => {
                "video_bit_rate DESC NULLS LAST, sort_name, id"
            }
            BaseItemOrder::AirTimeAscending => "sort_name ASC, id",
            BaseItemOrder::AirTimeDescending => "sort_name DESC, id",
            BaseItemOrder::StudioAscending => "studio ASC NULLS LAST, sort_name, id",
            BaseItemOrder::StudioDescending => "studio DESC NULLS LAST, sort_name, id",
            BaseItemOrder::IsFavoriteOrLikedAscending => "is_favorite_or_liked ASC, sort_name, id",
            BaseItemOrder::IsFavoriteOrLikedDescending => {
                "is_favorite_or_liked DESC, sort_name, id"
            }
            BaseItemOrder::DateLastContentAddedAscending => {
                "date_last_content_added ASC NULLS LAST, sort_name, id"
            }
            BaseItemOrder::DateLastContentAddedDescending => {
                "date_last_content_added DESC NULLS LAST, sort_name, id"
            }
            BaseItemOrder::ParentIndexNumberAscending => {
                "parent_index_number ASC NULLS LAST, sort_name, id"
            }
            BaseItemOrder::ParentIndexNumberDescending => {
                "parent_index_number DESC NULLS LAST, sort_name, id"
            }
            BaseItemOrder::IndexNumberAscending => "index_number ASC NULLS LAST, sort_name, id",
            BaseItemOrder::IndexNumberDescending => "index_number DESC NULLS LAST, sort_name, id",
            _ => unreachable!("extended-sort query only handles extended orders"),
        };
        self.query_raw_page(cte, values, "filtered", order, "ExtendedSort", query)
            .await
    }

    async fn query_by_filtered_cte(
        &self,
        query: &BaseItemQuery,
    ) -> Result<BaseItemPage, BaseItemError> {
        let (cte, values) = filtered_query_cte(query);
        let order = match query.order {
            BaseItemOrder::SortNameDescending => "sort_name DESC, id",
            BaseItemOrder::DateCreatedAscending => "date_created ASC, id",
            BaseItemOrder::DateCreatedDescending => "date_created DESC, id",
            BaseItemOrder::PremiereDateAscending => "premiere_date ASC NULLS LAST, sort_name, id",
            BaseItemOrder::PremiereDateDescending => "premiere_date DESC NULLS LAST, sort_name, id",
            BaseItemOrder::Random => "random(), id",
            _ => "sort_name, id",
        };
        self.query_raw_page(cte, values, "filtered", order, "FilteredItems", query)
            .await
    }

    /// Queries the next aired episode for each recently active series.
    ///
    /// Playback state is aggregated over every alternate version. The highest aired watched
    /// position establishes the starting point and each series contributes its first later
    /// unplayed primary episode. When configured, placed specials participate in the same aired
    /// ordering as the official server. Rewatching additionally contributes the first watched
    /// episode after the most recently played position. Series are ordered by their most recent
    /// version-level activity.
    ///
    /// # Errors
    ///
    /// Returns a database error when the query fails.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn next_up(
        &self,
        user_id: Uuid,
        query: &BaseItemQuery,
        enable_rewatching: bool,
        enable_resumable: bool,
        display_specials_within_seasons: bool,
        next_up_date_cutoff: Option<DateTime<Utc>>,
        start_index: u64,
        limit: Option<u64>,
    ) -> Result<BaseItemPage, BaseItemError> {
        let mut values = vec![user_id.into()];
        let mut sql = String::from(
            "WITH scoped_episodes AS MATERIALIZED (\
                 SELECT episode.* \
                 FROM jellyfin.base_items AS episode \
                 WHERE episode.item_type = 'Episode' \
                   AND episode.is_virtual_item = false \
                   AND episode.primary_version_id IS NULL \
                   AND episode.series_presentation_unique_key IS NOT NULL \
                   AND (episode.data ->> 'OwnerId' IS NULL \
                        OR episode.data ->> 'ExtraType' IS NOT NULL)",
        );
        if !display_specials_within_seasons {
            sql.push_str(" AND episode.parent_index_number <> 0");
        }
        if let Some(parent_id) = query.parent_id {
            values.push(parent_id.into());
            let series_types =
                quoted_string_list(&expand_item_type_aliases(&["Series".to_owned()]));
            let _ = write!(
                sql,
                " AND (episode.id IN (\
                     SELECT closure.item_id FROM jellyfin.ancestor_ids AS closure \
                     WHERE closure.parent_item_id = ${}\
                 ) OR EXISTS (\
                     SELECT 1 FROM jellyfin.base_items AS series_scope \
                     WHERE series_scope.id = ${} \
                       AND series_scope.item_type IN ({series_types}) \
                       AND episode.series_presentation_unique_key IN (\
                           NULLIF(series_scope.presentation_unique_key, ''), \
                           replace(series_scope.id::text, '-', '')\
                       )\
                 ))",
                values.len(),
                values.len()
            );
        }
        if !query.parent_ids.is_empty() {
            sql.push_str(
                " AND episode.id IN (SELECT closure.item_id \
                 FROM jellyfin.ancestor_ids AS closure WHERE 1 = 1",
            );
            append_uuid_list_filter(
                &mut sql,
                &mut values,
                "closure.parent_item_id",
                &query.parent_ids,
            );
            sql.push(')');
        }
        if let Some(condition) = policy_filter_sql("episode", query) {
            sql.push_str(" AND (");
            sql.push_str(&condition);
            sql.push(')');
        }
        sql.push_str(
            "), episode_user_state AS MATERIALIZED (\
                 SELECT episode.id AS episode_id, \
                        COALESCE(BOOL_OR(data.played), false) AS is_watched, \
                        COALESCE(BOOL_OR(data.playback_position_ticks > 0), false) AS is_resumable, \
                        MAX(data.last_played_date) AS last_played_date \
                 FROM scoped_episodes AS episode \
                 INNER JOIN jellyfin.base_items AS version \
                   ON COALESCE(version.primary_version_id, version.id) = episode.id \
                  AND version.item_type = 'Episode' \
                 LEFT JOIN jellyfin.user_data AS data \
                   ON data.item_id = version.id AND data.user_id = $1 \
                 GROUP BY episode.id\
             ), episode_state AS MATERIALIZED (\
                 SELECT episode.*, state.is_watched, state.is_resumable, \
                        state.last_played_date \
                 FROM scoped_episodes AS episode \
                 INNER JOIN episode_user_state AS state ON state.episode_id = episode.id\
             ), aired_episode_state AS MATERIALIZED (\
                 SELECT episode.*, \
                        CASE WHEN COALESCE(episode.parent_index_number, -1) = 0 \
                             THEN COALESCE(\
                                 NULLIF(episode.data ->> 'AirsAfterSeasonNumber', '')::integer, \
                                 NULLIF(episode.data ->> 'AirsBeforeSeasonNumber', '')::integer, \
                                 -1) \
                             ELSE COALESCE(episode.parent_index_number, -1) \
                        END AS aired_season, \
                        CASE WHEN COALESCE(episode.parent_index_number, -1) = 0 \
                                       AND episode.data ->> 'AirsAfterSeasonNumber' IS NOT NULL \
                             THEN 1 ELSE 0 END AS aired_after_season, \
                        CASE WHEN COALESCE(episode.parent_index_number, -1) = 0 \
                             THEN COALESCE(\
                                 NULLIF(episode.data ->> 'AirsBeforeEpisodeNumber', '')::integer, \
                                 0) \
                             ELSE COALESCE(episode.index_number, -1) \
                        END AS aired_episode, \
                        CASE WHEN COALESCE(episode.parent_index_number, -1) = 0 \
                             THEN 0 ELSE 1 END AS aired_item_kind, \
                        CASE WHEN COALESCE(episode.parent_index_number, -1) = 0 \
                             THEN COALESCE(episode.index_number, 0) ELSE 0 \
                        END AS aired_special_number \
                 FROM episode_state AS episode\
             ), regular_state AS MATERIALIZED (\
                 SELECT episode.* \
                 FROM aired_episode_state AS episode \
                 WHERE COALESCE(episode.parent_index_number, -1) <> 0\
             ), series_activity AS MATERIALIZED (\
                 SELECT episode.series_presentation_unique_key AS series_key, \
                        MAX(episode.last_played_date) AS last_played_date \
                 FROM regular_state AS episode \
                 GROUP BY episode.series_presentation_unique_key \
                 HAVING MAX(episode.last_played_date) IS NOT NULL",
        );
        if let Some(cutoff) = next_up_date_cutoff {
            values.push(cutoff.into());
            let _ = write!(
                sql,
                " AND MAX(episode.last_played_date) >= ${}",
                values.len()
            );
        }
        sql.push_str(
            "), last_watched AS MATERIALIZED (\
                 SELECT DISTINCT ON (episode.series_presentation_unique_key) \
                        episode.series_presentation_unique_key AS series_key, \
                        episode.id AS episode_id, \
                        episode.last_played_date \
                 FROM regular_state AS episode \
                 INNER JOIN series_activity AS activity \
                   ON activity.series_key = episode.series_presentation_unique_key \
                 WHERE episode.is_watched \
                 ORDER BY episode.series_presentation_unique_key, \
                          episode.aired_season DESC, \
                          episode.aired_after_season DESC, \
                          episode.aired_episode DESC, \
                          episode.aired_item_kind DESC, \
                          episode.premiere_date DESC NULLS LAST, \
                          episode.sort_name DESC, episode.id DESC\
             ), ranked_candidates AS (\
                 SELECT episode.id AS episode_id, \
                        ROW_NUMBER() OVER (\
                            PARTITION BY episode.series_presentation_unique_key \
                            ORDER BY episode.aired_season, \
                                     episode.aired_after_season, \
                                     episode.aired_episode, \
                                     episode.aired_item_kind, \
                                     episode.premiere_date NULLS LAST, \
                                     episode.sort_name, episode.id\
                        ) AS candidate_rank \
                 FROM regular_state AS episode \
                 INNER JOIN series_activity AS activity \
                   ON activity.series_key = episode.series_presentation_unique_key \
                 LEFT JOIN last_watched \
                   ON last_watched.series_key = episode.series_presentation_unique_key \
                 LEFT JOIN regular_state AS watched \
                   ON watched.id = last_watched.episode_id \
                 WHERE NOT episode.is_watched \
                   AND (last_watched.series_key IS NULL \
                        OR (episode.aired_season, episode.aired_after_season, \
                            episode.aired_episode, episode.aired_item_kind, \
                            COALESCE(episode.premiere_date, '-infinity'::timestamptz), \
                            episode.sort_name, episode.id) \
                           > (watched.aired_season, watched.aired_after_season, \
                              watched.aired_episode, watched.aired_item_kind, \
                              COALESCE(watched.premiere_date, '-infinity'::timestamptz), \
                              watched.sort_name, watched.id))\
             ), normal_considered AS MATERIALIZED (\
                 SELECT special.*, false AS is_marker \
                 FROM aired_episode_state AS special \
                 INNER JOIN series_activity AS activity \
                   ON activity.series_key = special.series_presentation_unique_key \
                 WHERE COALESCE(special.parent_index_number, -1) = 0 \
                   AND (special.data ->> 'AirsBeforeSeasonNumber' IS NOT NULL \
                        OR special.data ->> 'AirsAfterSeasonNumber' IS NOT NULL) \
                   AND NOT special.is_watched \
                 UNION ALL \
                 SELECT episode.*, false AS is_marker \
                 FROM ranked_candidates AS candidate \
                 INNER JOIN aired_episode_state AS episode \
                   ON episode.id = candidate.episode_id \
                 WHERE candidate.candidate_rank = 1 \
                 UNION ALL \
                 SELECT episode.*, true AS is_marker \
                 FROM last_watched \
                 INNER JOIN aired_episode_state AS episode \
                   ON episode.id = last_watched.episode_id\
             ), normal_ranked AS (\
                 SELECT considered.*, \
                        ROW_NUMBER() OVER (\
                            PARTITION BY considered.series_presentation_unique_key \
                            ORDER BY considered.aired_season, \
                                     considered.aired_after_season, \
                                     considered.aired_episode, \
                                     considered.aired_item_kind, \
                                     considered.aired_special_number, \
                                     considered.premiere_date NULLS LAST, \
                                     considered.sort_name, considered.id\
                        ) AS aired_rank \
                 FROM normal_considered AS considered\
             ), normal_bounded AS (\
                 SELECT ranked.*, \
                        MAX(ranked.aired_rank) FILTER (WHERE ranked.is_marker) OVER (\
                            PARTITION BY ranked.series_presentation_unique_key\
                        ) AS marker_rank \
                 FROM normal_ranked AS ranked\
             ), normal_eligible AS (\
                 SELECT bounded.*, \
                        ROW_NUMBER() OVER (\
                            PARTITION BY bounded.series_presentation_unique_key \
                            ORDER BY bounded.aired_rank\
                        ) AS candidate_rank \
                 FROM normal_bounded AS bounded \
                 WHERE NOT bounded.is_marker \
                   AND (bounded.marker_rank IS NULL \
                        OR bounded.aired_rank > bounded.marker_rank)\
             ), normal_selected AS MATERIALIZED (\
                 SELECT item.*, last_watched.last_played_date AS series_last_played_date, \
                        0 AS result_rank \
                 FROM normal_eligible AS candidate \
                 INNER JOIN jellyfin.base_items AS item ON item.id = candidate.id \
                 LEFT JOIN last_watched \
                   ON last_watched.series_key = candidate.series_presentation_unique_key \
                 WHERE candidate.candidate_rank = 1",
        );
        if !enable_resumable {
            sql.push_str(" AND NOT candidate.is_resumable");
        }
        sql.push(')');
        if enable_rewatching {
            sql.push_str(
                ", last_watched_for_rewatching AS MATERIALIZED (\
                     SELECT DISTINCT ON (episode.series_presentation_unique_key) \
                            episode.series_presentation_unique_key AS series_key, \
                            episode.id AS episode_id, \
                            episode.last_played_date \
                     FROM regular_state AS episode \
                     INNER JOIN series_activity AS activity \
                       ON activity.series_key = episode.series_presentation_unique_key \
                     WHERE episode.is_watched \
                     ORDER BY episode.series_presentation_unique_key, \
                              episode.last_played_date DESC NULLS LAST, \
                              episode.aired_season DESC, \
                              episode.aired_after_season DESC, \
                              episode.aired_episode DESC, \
                              episode.aired_item_kind DESC, \
                              episode.premiere_date DESC NULLS LAST, \
                              episode.sort_name DESC, episode.id DESC\
                 ), ranked_rewatch_candidates AS (\
                     SELECT episode.id AS episode_id, \
                            ROW_NUMBER() OVER (\
                                PARTITION BY episode.series_presentation_unique_key \
                                ORDER BY episode.aired_season, \
                                         episode.aired_after_season, \
                                         episode.aired_episode, \
                                         episode.aired_item_kind, \
                                         episode.premiere_date NULLS LAST, \
                                         episode.sort_name, episode.id\
                            ) AS candidate_rank \
                     FROM regular_state AS episode \
                     INNER JOIN last_watched_for_rewatching AS last_watched \
                       ON last_watched.series_key = episode.series_presentation_unique_key \
                     INNER JOIN regular_state AS watched \
                       ON watched.id = last_watched.episode_id \
                     WHERE episode.is_watched \
                       AND (episode.aired_season, episode.aired_after_season, \
                            episode.aired_episode, episode.aired_item_kind, \
                            COALESCE(episode.premiere_date, '-infinity'::timestamptz), \
                            episode.sort_name, episode.id) \
                           > (watched.aired_season, watched.aired_after_season, \
                              watched.aired_episode, watched.aired_item_kind, \
                              COALESCE(watched.premiere_date, '-infinity'::timestamptz), \
                              watched.sort_name, watched.id)\
                 ), rewatch_considered AS MATERIALIZED (\
                     SELECT special.*, false AS is_marker \
                     FROM aired_episode_state AS special \
                     INNER JOIN series_activity AS activity \
                       ON activity.series_key = special.series_presentation_unique_key \
                     WHERE COALESCE(special.parent_index_number, -1) = 0 \
                       AND (special.data ->> 'AirsBeforeSeasonNumber' IS NOT NULL \
                            OR special.data ->> 'AirsAfterSeasonNumber' IS NOT NULL) \
                     UNION ALL \
                     SELECT episode.*, false AS is_marker \
                     FROM ranked_rewatch_candidates AS candidate \
                     INNER JOIN aired_episode_state AS episode \
                       ON episode.id = candidate.episode_id \
                     WHERE candidate.candidate_rank = 1 \
                     UNION ALL \
                     SELECT episode.*, true AS is_marker \
                     FROM last_watched_for_rewatching \
                     INNER JOIN aired_episode_state AS episode \
                       ON episode.id = last_watched_for_rewatching.episode_id\
                 ), rewatch_ranked AS (\
                     SELECT considered.*, \
                            ROW_NUMBER() OVER (\
                                PARTITION BY considered.series_presentation_unique_key \
                                ORDER BY considered.aired_season, \
                                         considered.aired_after_season, \
                                         considered.aired_episode, \
                                         considered.aired_item_kind, \
                                         considered.aired_special_number, \
                                         considered.premiere_date NULLS LAST, \
                                         considered.sort_name, considered.id\
                            ) AS aired_rank \
                     FROM rewatch_considered AS considered\
                 ), rewatch_bounded AS (\
                     SELECT ranked.*, \
                            MAX(ranked.aired_rank) FILTER (WHERE ranked.is_marker) OVER (\
                                PARTITION BY ranked.series_presentation_unique_key\
                            ) AS marker_rank \
                     FROM rewatch_ranked AS ranked\
                 ), rewatch_eligible AS (\
                     SELECT bounded.*, \
                            ROW_NUMBER() OVER (\
                                PARTITION BY bounded.series_presentation_unique_key \
                                ORDER BY bounded.aired_rank\
                            ) AS candidate_rank \
                     FROM rewatch_bounded AS bounded \
                     WHERE NOT bounded.is_marker \
                       AND (bounded.marker_rank IS NULL \
                            OR bounded.aired_rank > bounded.marker_rank)\
                 ), rewatch_selected AS MATERIALIZED (\
                     SELECT item.*, last_watched.last_played_date AS series_last_played_date, \
                            1 AS result_rank \
                     FROM rewatch_eligible AS candidate \
                     INNER JOIN jellyfin.base_items AS item ON item.id = candidate.id \
                     INNER JOIN last_watched_for_rewatching AS last_watched \
                       ON last_watched.series_key = candidate.series_presentation_unique_key \
                     WHERE candidate.candidate_rank = 1 \
                       AND NOT candidate.is_resumable\
                 ), selected AS MATERIALIZED (\
                     SELECT * FROM normal_selected \
                     UNION ALL \
                     SELECT * FROM rewatch_selected\
                 )",
            );
        } else {
            sql.push_str(", selected AS MATERIALIZED (SELECT * FROM normal_selected)");
        }
        let total = if query.enable_total_record_count.unwrap_or(false) {
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
             ORDER BY series_last_played_date DESC NULLS LAST, \
                      result_rank, \
                      series_presentation_unique_key, id"
        );
        push_bind(
            &mut page_sql,
            &mut page_values,
            i64::try_from(start_index).unwrap_or(i64::MAX),
            " OFFSET ",
        );
        if let Some(limit) = limit.filter(|limit| *limit > 0) {
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
        .all(self.database.as_ref())
        .await?;
        Ok(BaseItemPage {
            total_record_count: total
                .map(|count| u64::try_from(count).unwrap_or_default())
                .unwrap_or_default(),
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
            total_record_count: count.map_or_else(
                || u64::try_from(items.len()).unwrap_or(u64::MAX),
                |count| u64::try_from(count).unwrap_or_default(),
            ),
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
                && let Some(updated) = updated
            {
                result.push(updated);
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
            .all(self.database.as_ref())
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
            .all(self.database.as_ref())
            .await?;
        hierarchy_entries(closure, false, self.database.as_ref()).await
    }

    /// Resolves the nearest ancestor of any requested type for each item in one query.
    ///
    /// The closure-table depth, rather than the direct parent, defines proximity so
    /// callers can find media containers through arbitrary intermediate folders.
    ///
    /// # Errors
    ///
    /// Returns a database error when the ancestor lookup fails.
    pub async fn nearest_ancestor_ids_by_type(
        &self,
        item_ids: &[Uuid],
        ancestor_item_types: &[String],
    ) -> Result<HashMap<Uuid, Uuid>, BaseItemError> {
        if item_ids.is_empty() || ancestor_item_types.is_empty() {
            return Ok(HashMap::new());
        }
        let ancestor_item_types = expand_item_type_aliases(ancestor_item_types);
        let mut values = Vec::<SeaValue>::new();
        let mut sql = String::from(
            "SELECT DISTINCT ON (closure.item_id) \
                    closure.item_id, closure.parent_item_id AS ancestor_id \
             FROM jellyfin.ancestor_ids AS closure \
             JOIN jellyfin.base_items AS ancestor \
               ON ancestor.id = closure.parent_item_id \
             WHERE true",
        );
        append_uuid_list_filter(&mut sql, &mut values, "closure.item_id", item_ids);
        append_string_list_filter(
            &mut sql,
            &mut values,
            "ancestor.item_type",
            &ancestor_item_types,
            false,
        );
        sql.push_str(" ORDER BY closure.item_id, closure.depth ASC, closure.parent_item_id ASC");
        Ok(
            NearestAncestorId::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .all(self.database.as_ref())
            .await?
            .into_iter()
            .map(|row| (row.item_id, row.ancestor_id))
            .collect(),
        )
    }

    /// Returns requested items that are collection folders or descendants of
    /// one of the selected collection folders.
    ///
    /// The folder type check is deliberate: user deletion preferences contain
    /// identifiers, but only real `CollectionFolder` rows grant scoped delete
    /// access. The closure-table join keeps this one set-based query for a DTO
    /// page instead of walking every item's ancestors.
    ///
    /// # Errors
    ///
    /// Returns a database error when the membership query fails.
    pub async fn item_ids_in_collection_folders(
        &self,
        item_ids: &[Uuid],
        collection_folder_ids: &[Uuid],
    ) -> Result<HashSet<Uuid>, BaseItemError> {
        if item_ids.is_empty() || collection_folder_ids.is_empty() {
            return Ok(HashSet::new());
        }

        let collection_folder_types = expand_item_type_aliases(&["CollectionFolder".to_owned()]);
        let mut sql = String::from(
            "WITH requested AS MATERIALIZED (\
                 SELECT item.id FROM jellyfin.base_items AS item WHERE true",
        );
        let mut values = Vec::<SeaValue>::new();
        append_uuid_list_filter(&mut sql, &mut values, "item.id", item_ids);
        sql.push_str(
            "), allowed_folders AS MATERIALIZED (\
                 SELECT folder.id FROM jellyfin.base_items AS folder WHERE true",
        );
        append_uuid_list_filter(&mut sql, &mut values, "folder.id", collection_folder_ids);
        append_string_list_filter(
            &mut sql,
            &mut values,
            "folder.item_type",
            &collection_folder_types,
            false,
        );
        sql.push_str(
            ") SELECT requested.id AS item_id \
               FROM requested \
               INNER JOIN allowed_folders ON allowed_folders.id = requested.id \
               UNION \
               SELECT closure.item_id \
               FROM jellyfin.ancestor_ids AS closure \
               INNER JOIN requested ON requested.id = closure.item_id \
               INNER JOIN allowed_folders ON allowed_folders.id = closure.parent_item_id",
        );

        Ok(ItemIdRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .all(self.database.as_ref())
        .await?
        .into_iter()
        .map(|row| row.item_id)
        .collect())
    }

    /// Computes official 24-hour latest-TV grouping decisions for the top series.
    ///
    /// Eligible episodes come from the same filtered query as the latest-media page.
    /// The top series are selected before the result limit, while complete season and
    /// series counts use real primary rows. The query emits one bounded projection per
    /// series and never loads episode collections.
    ///
    /// # Errors
    ///
    /// Returns a database error when the grouping query fails.
    #[allow(
        clippy::too_many_lines,
        reason = "the grouping is one set-based SQL query"
    )]
    pub async fn latest_tv_groups(
        &self,
        query: &BaseItemQuery,
        limit: u64,
    ) -> Result<Vec<LatestTvGroup>, BaseItemError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let (mut sql, mut values) = filtered_query_cte(query);
        let episode_types = quoted_string_list(&expand_item_type_aliases(&["Episode".to_owned()]));
        let season_types = quoted_string_list(&expand_item_type_aliases(&["Season".to_owned()]));
        values.push(i64::try_from(limit).unwrap_or(i64::MAX).into());
        let limit_parameter = values.len();
        let _ = write!(
            sql,
            ", eligible_episode_groups AS MATERIALIZED (\
                 SELECT episode.id, episode.series_id, episode.season_id, episode.date_created, \
                        COALESCE(NULLIF(episode.data ->> 'SeriesName', ''), \
                                 NULLIF(series.name, ''), episode.series_id::text) \
                            AS series_group_key \
                 FROM filtered AS episode \
                 LEFT JOIN jellyfin.base_items AS series ON series.id = episode.series_id \
                 WHERE episode.item_type IN ({episode_types}) \
                   AND episode.primary_version_id IS NULL \
                   AND episode.is_virtual_item = false \
                   AND episode.series_id IS NOT NULL\
             ), top_series AS MATERIALIZED (\
                 SELECT episode.series_group_key, MAX(episode.date_created) AS max_date \
                 FROM eligible_episode_groups AS episode \
                 GROUP BY episode.series_group_key \
                 ORDER BY max_date DESC, episode.series_group_key DESC \
                 LIMIT ${limit_parameter}\
             ), eligible_episodes AS MATERIALIZED (\
                 SELECT episode.* \
                 FROM eligible_episode_groups AS episode \
                 JOIN top_series AS candidate \
                   ON candidate.series_group_key = episode.series_group_key\
             ), series_max AS (\
                 SELECT episode.series_group_key, MAX(episode.date_created) AS max_date \
                 FROM eligible_episodes AS episode \
                 GROUP BY episode.series_group_key\
             ), recent_episodes AS MATERIALIZED (\
                 SELECT episode.*, latest.max_date \
                 FROM eligible_episodes AS episode \
                 JOIN series_max AS latest \
                   ON latest.series_group_key = episode.series_group_key \
                 WHERE episode.date_created >= latest.max_date - INTERVAL '24 hours'\
             ), recent_stats AS (\
                 SELECT recent.series_group_key, \
                        (ARRAY_AGG(recent.series_id \
                            ORDER BY recent.date_created DESC, recent.id DESC))[1] AS series_id, \
                        MAX(recent.max_date) AS max_date, \
                        (ARRAY_AGG(recent.id ORDER BY recent.date_created DESC, recent.id DESC))[1] \
                            AS most_recent_episode_id, \
                        COUNT(*)::bigint AS recent_child_count, \
                        COUNT(DISTINCT recent.season_id)::bigint AS recent_season_count, \
                        (ARRAY_AGG(recent.season_id ORDER BY recent.season_id) \
                            FILTER (WHERE recent.season_id IS NOT NULL))[1] AS recent_season_id \
                 FROM recent_episodes AS recent \
                 GROUP BY recent.series_group_key\
             ), season_episode_counts AS (\
                 SELECT episode.season_id, COUNT(*)::bigint AS episode_count \
                 FROM jellyfin.base_items AS episode \
                 WHERE episode.item_type IN ({episode_types}) \
                   AND episode.primary_version_id IS NULL \
                   AND episode.is_virtual_item = false \
                   AND (episode.data ->> 'OwnerId' IS NULL \
                        OR episode.data ->> 'ExtraType' IS NOT NULL) \
                   AND episode.season_id IN (\
                       SELECT DISTINCT recent.season_id \
                       FROM recent_episodes AS recent \
                       WHERE recent.season_id IS NOT NULL\
                   ) \
                 GROUP BY episode.season_id\
             ), series_season_counts AS (\
                 SELECT COALESCE(season.series_id, season.parent_id) AS series_id, \
                        COUNT(*)::bigint AS season_count \
                 FROM jellyfin.base_items AS season \
                 JOIN (SELECT DISTINCT series_id FROM recent_stats) AS candidate \
                   ON candidate.series_id = COALESCE(season.series_id, season.parent_id) \
                 WHERE season.item_type IN ({season_types}) \
                   AND season.primary_version_id IS NULL \
                   AND season.is_virtual_item = false \
                   AND (season.data ->> 'OwnerId' IS NULL \
                        OR season.data ->> 'ExtraType' IS NOT NULL) \
                 GROUP BY COALESCE(season.series_id, season.parent_id)\
             ) \
             SELECT stats.series_id, stats.most_recent_episode_id, stats.max_date, \
                    stats.recent_child_count, \
                    CASE \
                      WHEN stats.recent_season_count > 1 THEN stats.series_id \
                      WHEN stats.recent_season_count = 1 \
                       AND (stats.recent_child_count > 1 \
                            OR stats.recent_child_count = COALESCE(season_totals.episode_count, 0)) \
                       AND COALESCE(series_totals.season_count, 1) > 1 \
                        THEN stats.recent_season_id \
                      WHEN stats.recent_season_count = 1 \
                       AND (stats.recent_child_count > 1 \
                            OR stats.recent_child_count = COALESCE(season_totals.episode_count, 0)) \
                        THEN stats.series_id \
                      ELSE NULL \
                    END AS selected_container_id \
             FROM recent_stats AS stats \
             LEFT JOIN season_episode_counts AS season_totals \
               ON season_totals.season_id = stats.recent_season_id \
             LEFT JOIN series_season_counts AS series_totals \
               ON series_totals.series_id = stats.series_id \
             ORDER BY stats.max_date DESC, stats.series_id DESC"
        );
        Ok(
            LatestTvGroup::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .all(self.database.as_ref())
            .await?,
        )
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
            .all(self.database.as_ref())
            .await?;
        hierarchy_entries(closure, true, self.database.as_ref()).await
    }

    /// Repairs fallback episode names from an established original title in one
    /// set-based update scoped to a series or a single episode.
    ///
    /// The update deliberately runs after metadata providers. It only touches
    /// visible primary episodes whose name is still empty, series-derived, or
    /// path-derived, and honors a case-insensitive `LockedFields.Name`.
    ///
    /// # Errors
    ///
    /// Returns a database error when the update cannot be executed.
    #[allow(clippy::too_many_lines)] // The set-based SQL is intentionally kept as one atomic statement.
    pub async fn repair_episode_titles_from_original(
        &self,
        scope_id: Uuid,
    ) -> Result<u64, BaseItemError> {
        let result = self
            .database
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                WITH scope AS MATERIALIZED (
                    SELECT item_type
                    FROM jellyfin.base_items
                    WHERE id = $1
                ),
                candidate_ids AS (
                    SELECT episode.id
                    FROM scope
                    JOIN jellyfin.base_items AS episode
                      ON episode.series_id = $1
                    WHERE scope.item_type = 'Series'
                      AND episode.item_type = 'Episode'
                      AND episode.primary_version_id IS NULL
                    UNION ALL
                    SELECT episode.id
                    FROM scope
                    JOIN jellyfin.base_items AS episode
                      ON episode.id = $1
                    WHERE scope.item_type = 'Episode'
                      AND episode.item_type = 'Episode'
                      AND episode.primary_version_id IS NULL
                ),
                raw AS (
                    SELECT episode.id,
                           episode.name,
                           NULLIF(btrim(series.name), '') AS series_title,
                           NULLIF(btrim(metadata_series.value #>> '{}'), '') AS metadata_series_title,
                           original.value #>> '{}' AS original_title,
                           NULLIF(btrim(original.value #>> '{}'), '') AS normalized_original_title,
                           NULLIF(
                               btrim(
                                   CASE
                                       WHEN path_part.file_name ~ '^\.[^.]+$'
                                           THEN path_part.file_name
                                       ELSE regexp_replace(path_part.file_name, '\.[^.]*$', '')
                                   END
                               ),
                               ''
                           ) AS path_title,
                           locked.value AS locked_fields
                    FROM candidate_ids
                    JOIN jellyfin.base_items AS episode ON episode.id = candidate_ids.id
                    LEFT JOIN jellyfin.base_items AS series ON series.id = episode.series_id
                    CROSS JOIN LATERAL (
                        SELECT regexp_replace(COALESCE(episode.path, ''), '^.*/', '') AS file_name
                    ) AS path_part
                    LEFT JOIN LATERAL (
                        SELECT entry.value
                        FROM jsonb_each(
                            CASE
                                WHEN jsonb_typeof(episode.data) = 'object' THEN episode.data
                                ELSE '{}'::jsonb
                            END
                        ) AS entry(key, value)
                        WHERE lower(entry.key) = 'originaltitle'
                          AND jsonb_typeof(entry.value) = 'string'
                        LIMIT 1
                    ) AS original ON TRUE
                    LEFT JOIN LATERAL (
                        SELECT entry.value
                        FROM jsonb_each(
                            CASE
                                WHEN jsonb_typeof(episode.data) = 'object' THEN episode.data
                                ELSE '{}'::jsonb
                            END
                        ) AS entry(key, value)
                        WHERE lower(entry.key) = 'seriesname'
                          AND jsonb_typeof(entry.value) = 'string'
                        LIMIT 1
                    ) AS metadata_series ON TRUE
                    LEFT JOIN LATERAL (
                        SELECT entry.value
                        FROM jsonb_each(
                            CASE
                                WHEN jsonb_typeof(episode.data) = 'object' THEN episode.data
                                ELSE '{}'::jsonb
                            END
                        ) AS entry(key, value)
                        WHERE lower(entry.key) = 'lockedfields'
                        LIMIT 1
                    ) AS locked ON TRUE
                ),
                eligible AS (
                    SELECT id, original_title
                    FROM raw
                    WHERE normalized_original_title IS NOT NULL
                      AND NOT CASE jsonb_typeof(locked_fields)
                          WHEN 'array' THEN EXISTS (
                              SELECT 1
                              FROM jsonb_array_elements_text(locked_fields) AS field(value)
                              WHERE lower(field.value) = 'name'
                          )
                          WHEN 'string' THEN EXISTS (
                              SELECT 1
                              FROM regexp_split_to_table(locked_fields #>> '{}', '[|,]') AS field(value)
                              WHERE lower(btrim(field.value)) = 'name'
                          )
                          ELSE FALSE
                      END
                      AND (
                          name IS NULL
                          OR btrim(name) = ''
                          OR (series_title IS NOT NULL AND lower(btrim(name)) = lower(series_title))
                          OR (metadata_series_title IS NOT NULL AND lower(btrim(name)) = lower(metadata_series_title))
                          OR (path_title IS NOT NULL AND lower(btrim(name)) = lower(path_title))
                      )
                      AND (series_title IS NULL OR lower(normalized_original_title) <> lower(series_title))
                      AND (
                          metadata_series_title IS NULL
                          OR lower(normalized_original_title) <> lower(metadata_series_title)
                      )
                      AND (path_title IS NULL OR lower(normalized_original_title) <> lower(path_title))
                )
                UPDATE jellyfin.base_items AS episode
                SET name = eligible.original_title,
                    sort_name = eligible.original_title
                FROM eligible
                WHERE episode.id = eligible.id
                ",
                [scope_id.into()],
            ))
            .await?;
        Ok(result.rows_affected())
    }

    /// Loads the minimal descendant projection needed for scan reconciliation.
    ///
    /// # Errors
    ///
    /// Returns a database error when the query fails.
    pub async fn descendant_scan_candidates(
        &self,
        id: Uuid,
    ) -> Result<Vec<DescendantScanCandidate>, BaseItemError> {
        Ok(
            DescendantScanCandidate::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                SELECT item.id, item.item_type, item.path, item.parent_id, item.season_id
                FROM jellyfin.ancestor_ids AS closure
                JOIN jellyfin.base_items AS item ON item.id = closure.item_id
                WHERE closure.parent_item_id = $1
                ORDER BY closure.depth, closure.item_id
                ",
                [id.into()],
            ))
            .all(self.database.as_ref())
            .await?,
        )
    }

    /// Loads the minimal TV descendants needed to repair season hierarchy.
    ///
    /// Episode media paths and all large metadata fields are deliberately
    /// excluded because hierarchy reconciliation never reads them.
    ///
    /// # Errors
    ///
    /// Returns a database error when the query fails.
    pub async fn tv_hierarchy_candidates(
        &self,
        id: Uuid,
    ) -> Result<Vec<TvHierarchyCandidate>, BaseItemError> {
        Ok(
            TvHierarchyCandidate::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                SELECT item.id,
                       item.row_version,
                       item.item_type,
                       CASE WHEN item.item_type = 'Season' THEN item.path END AS path,
                       item.parent_id,
                       item.index_number,
                       item.parent_index_number,
                       item.series_id,
                       item.season_id,
                       item.series_presentation_unique_key
                FROM jellyfin.ancestor_ids AS closure
                JOIN jellyfin.base_items AS item ON item.id = closure.item_id
                WHERE closure.parent_item_id = $1
                  AND item.item_type IN ('Series', 'Season', 'Episode')
                ORDER BY closure.depth, closure.item_id
                ",
                [id.into()],
            ))
            .all(self.database.as_ref())
            .await?,
        )
    }

    /// Updates only an episode's season hierarchy fields.
    ///
    /// This preserves optimistic locking and hierarchy validation without
    /// loading the episode's full metadata row into application memory.
    ///
    /// # Errors
    ///
    /// Returns hierarchy, stale-version, not-found, or database errors.
    pub async fn reconcile_tv_episode_hierarchy(
        &self,
        id: Uuid,
        parent_id: Uuid,
        season_id: Uuid,
        series_presentation_unique_key: String,
        expected_row_version: i64,
    ) -> Result<(), BaseItemError> {
        let transaction = self.database.begin().await?;
        acquire_hierarchy_lock(&transaction).await?;
        let current = BaseItemRowVersion::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT row_version FROM jellyfin.base_items WHERE id = $1",
            [id.into()],
        ))
        .one(&transaction)
        .await?
        .ok_or(BaseItemError::NotFound)?;
        if current.row_version != expected_row_version {
            return Err(BaseItemError::StaleVersion);
        }
        validate_parent(&transaction, id, Some(parent_id)).await?;
        let result = base_item::Entity::update_many()
            .set(base_item::ActiveModel {
                parent_id: Set(Some(parent_id)),
                season_id: Set(Some(season_id)),
                series_presentation_unique_key: Set(Some(series_presentation_unique_key)),
                ..Default::default()
            })
            .filter(base_item::Column::Id.eq(id))
            .filter(base_item::Column::RowVersion.eq(expected_row_version))
            .exec(&transaction)
            .await
            .map_err(map_database_error)?;
        if result.rows_affected == 0 {
            return Err(BaseItemError::StaleVersion);
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Loads movie, series, and episode rows that have no overview or `TMDb` id,
    /// plus primary episodes whose title is empty or still matches their series.
    ///
    /// The maintenance query deliberately bypasses user policy filtering and
    /// returns only the identifiers needed by metadata-refresh scheduling.
    /// When `parent_id` is present, all descendants are considered, matching a
    /// recursive [`BaseItemQuery`].
    ///
    /// # Errors
    ///
    /// Returns a database error when the query fails.
    pub async fn missing_metadata_refresh_candidates(
        &self,
        parent_id: Option<Uuid>,
    ) -> Result<Vec<MetadataRefreshCandidate>, BaseItemError> {
        let mut values = Vec::new();
        let mut sql = String::from(
            r"
            SELECT item.id, item.item_type, item.series_id
            FROM jellyfin.base_items AS item
            WHERE item.item_type IN ('Series', 'Movie', 'Episode')
              AND item.primary_version_id IS NULL
              AND (item.data ->> 'OwnerId' IS NULL OR item.data ->> 'ExtraType' IS NOT NULL)
              AND (
                    item.overview IS NULL
                    OR btrim(item.overview) = ''
                    OR (
                        item.item_type = 'Episode'
                        AND (
                            item.name IS NULL
                            OR btrim(item.name) = ''
                            OR EXISTS (
                                SELECT 1
                                FROM jellyfin.base_items AS series
                                WHERE series.id = item.series_id
                                  AND lower(btrim(series.name)) = lower(btrim(item.name))
                            )
                        )
                    )
                    OR NOT EXISTS (
                        SELECT 1
                        FROM jsonb_each_text(
                            CASE
                                WHEN jsonb_typeof(item.data -> 'ProviderIds') = 'object'
                                THEN item.data -> 'ProviderIds'
                                ELSE '{}'::jsonb
                            END
                        ) AS provider(provider_id, provider_value)
                        WHERE lower(provider_id) = 'tmdb'
                    )
              )
            ",
        );
        if let Some(parent_id) = parent_id {
            push_bind(
                &mut sql,
                &mut values,
                parent_id,
                " AND EXISTS (\
                    SELECT 1 FROM jellyfin.ancestor_ids AS closure \
                    WHERE closure.item_id = item.id AND closure.parent_item_id = ",
            );
            sql.push(')');
        }
        sql.push_str(
            " ORDER BY CASE item.item_type \
                WHEN 'Series' THEN 0 WHEN 'Episode' THEN 1 ELSE 2 END, item.id",
        );
        Ok(
            MetadataRefreshCandidate::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .all(self.database.as_ref())
            .await?,
        )
    }

    /// Loads movie and series identifiers that have a `TMDb` identifier but no
    /// persisted primary image.
    ///
    /// The correlated anti-join mirrors Jellyfin's default image refresh,
    /// which downloads missing image types without replacing existing ones.
    /// Only identifiers are returned so scheduling a large image repair does
    /// not retain complete base-item or image rows in memory.
    ///
    /// # Errors
    ///
    /// Returns a database error when the query fails.
    pub async fn missing_primary_image_refresh_candidates(
        &self,
        parent_id: Option<Uuid>,
    ) -> Result<Vec<MetadataRefreshCandidate>, BaseItemError> {
        let mut values = Vec::new();
        let mut sql = String::from(
            r"
            SELECT item.id, item.item_type, NULL::uuid AS series_id
            FROM jellyfin.base_items AS item
            WHERE item.item_type IN ('Movie', 'Series')
              AND item.primary_version_id IS NULL
              AND EXISTS (
                    SELECT 1
                    FROM jsonb_each_text(
                        CASE
                            WHEN jsonb_typeof(item.data -> 'ProviderIds') = 'object'
                            THEN item.data -> 'ProviderIds'
                            ELSE '{}'::jsonb
                        END
                    ) AS provider(provider_id, provider_value)
                    WHERE lower(provider_id) = 'tmdb'
                      AND btrim(provider_value) ~ '^[1-9][0-9]*$'
              )
              AND NOT EXISTS (
                    SELECT 1
                    FROM jellyfin.base_item_images AS image
                    WHERE image.item_id = item.id
                      AND image.image_type = 0
              )
            ",
        );
        if let Some(parent_id) = parent_id {
            push_bind(
                &mut sql,
                &mut values,
                parent_id,
                " AND EXISTS (\
                    SELECT 1 FROM jellyfin.ancestor_ids AS closure \
                    WHERE closure.item_id = item.id AND closure.parent_item_id = ",
            );
            sql.push(')');
        }
        sql.push_str(" ORDER BY item.id");
        Ok(
            MetadataRefreshCandidate::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .all(self.database.as_ref())
            .await?,
        )
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

        let mut values = vec![
            item_id.into(),
            i64::try_from(start_index).unwrap_or(i64::MAX).into(),
        ];
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
            values.push(i64::try_from(limit).unwrap_or(i64::MAX).into());
            sql.push_str(" LIMIT $3");
        }

        let items = base_item::Model::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .all(self.database.as_ref())
        .await?;
        Ok(BaseItemPage {
            items,
            total_record_count: u64::try_from(total_record_count).unwrap_or(0),
            start_index,
        })
    }

    /// Searches persisted items with Jellyfin's database-provider scoring.
    ///
    /// Exact clean-name matches score highest, followed by prefix, word-prefix,
    /// and contains matches. Original-title matches fall back to the contains
    /// tier, mirroring `SqlSearchProvider`.
    ///
    /// # Errors
    ///
    /// Returns a database error when the scored query fails.
    pub async fn search(&self, query: &BaseItemQuery) -> Result<ScoredBaseItemPage, BaseItemError> {
        let raw_search_term = query
            .search_term
            .as_deref()
            .map(str::trim)
            .filter(|term| !term.is_empty())
            .unwrap_or_default();
        let clean_search_term = raw_search_term.clean_value();
        if raw_search_term.is_empty() || clean_search_term.is_empty() {
            return Ok(ScoredBaseItemPage {
                items: Vec::new(),
                total_record_count: 0,
                start_index: query.start_index,
            });
        }

        let clean_prefix = format!("{clean_search_term} ");
        let like_original = format!("%{raw_search_term}%");
        let (cte, count_values) =
            scored_search_cte(query, &clean_search_term, &clean_prefix, &like_original);
        let total_record_count = self
            .database
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!("{cte} SELECT COUNT(*)::bigint AS total_record_count FROM filtered"),
                count_values,
            ))
            .await?
            .map(|row| row.try_get::<i64>("", "total_record_count"))
            .transpose()?
            .unwrap_or(0);

        let limit = query.limit.unwrap_or(100);
        let (cte, mut page_values) =
            scored_search_cte(query, &clean_search_term, &clean_prefix, &like_original);
        page_values.push(i64::try_from(query.start_index).unwrap_or(i64::MAX).into());
        let mut page_sql = format!(
            "{cte} SELECT {BASE_ITEM_COLUMNS} FROM filtered \
             ORDER BY search_score DESC, id ASC OFFSET ${}",
            page_values.len()
        );
        page_values.push(i64::try_from(limit).unwrap_or(i64::MAX).into());
        write!(page_sql, " LIMIT ${}", page_values.len()).expect("writing to a String cannot fail");

        let items = base_item::Model::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            page_sql,
            page_values,
        ))
        .all(self.database.as_ref())
        .await?;
        Ok(ScoredBaseItemPage {
            items: items
                .into_iter()
                .map(|item| {
                    let score = search_score(
                        item.clean_name.as_deref(),
                        &clean_search_term,
                        &clean_prefix,
                    );
                    ScoredBaseItem { item, score }
                })
                .collect(),
            total_record_count: u64::try_from(total_record_count).unwrap_or(0),
            start_index: query.start_index,
        })
    }
}

fn linked_alternate_version_merge_sql(item_ids: &[Uuid]) -> (String, Vec<SeaValue>) {
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
             INNER JOIN requested_distinct AS requested ON requested.id = item.id\
         ), merge_groups AS MATERIALIZED (\
             SELECT DISTINCT group_id FROM requested_items\
         ), merge_members AS MATERIALIZED (\
             SELECT item.id, merge_groups.group_id \
             FROM jellyfin.base_items AS item \
             INNER JOIN merge_groups \
               ON item.id = merge_groups.group_id \
               OR item.primary_version_id = merge_groups.group_id\
         ), primary_version AS MATERIALIZED (\
             SELECT group_id AS id FROM merge_groups ORDER BY group_id LIMIT 1\
         ), inferred_local_links AS (\
             INSERT INTO jellyfin.linked_children \
                 (parent_id, child_id, child_type, sort_order) \
             SELECT member.group_id, member.id, 2, NULL \
             FROM merge_members AS member \
             WHERE member.id <> member.group_id \
               AND NOT EXISTS (\
                   SELECT 1 FROM jellyfin.linked_children AS link \
                   WHERE link.child_id = member.id AND link.child_type IN (2, 3)\
               ) \
             ON CONFLICT (parent_id, child_id) DO NOTHING \
             RETURNING child_id\
         ), subgroup_roots AS MATERIALIZED (\
             SELECT group_id AS id FROM merge_groups \
             UNION \
             SELECT link.child_id \
             FROM jellyfin.linked_children AS link \
             INNER JOIN merge_members AS member ON member.id = link.child_id \
             WHERE link.child_type = 3\
         ), removed_linked_versions AS (\
             DELETE FROM jellyfin.linked_children AS link \
             WHERE link.child_type = 3 \
               AND link.child_id IN (SELECT id FROM merge_members) \
               AND link.parent_id IN (SELECT id FROM merge_members) \
               AND (SELECT COUNT(*) FROM inferred_local_links) >= 0 \
             RETURNING link.child_id\
         ), linked_versions AS (\
             INSERT INTO jellyfin.linked_children \
                 (parent_id, child_id, child_type, sort_order) \
             SELECT primary_version.id, subgroup.id, 3, NULL \
             FROM primary_version \
             CROSS JOIN subgroup_roots AS subgroup \
             WHERE subgroup.id <> primary_version.id \
               AND (SELECT COUNT(*) FROM removed_linked_versions) >= 0 \
             ON CONFLICT (parent_id, child_id) \
             DO UPDATE SET child_type = 3 \
             RETURNING child_id\
         ), updated AS (\
             UPDATE jellyfin.base_items AS item \
             SET primary_version_id = CASE \
                     WHEN item.id = (SELECT id FROM primary_version) THEN NULL \
                     ELSE (SELECT id FROM primary_version) \
                 END \
             WHERE item.id IN (SELECT id FROM merge_members) \
               AND item.primary_version_id IS DISTINCT FROM CASE \
                     WHEN item.id = (SELECT id FROM primary_version) THEN NULL \
                     ELSE (SELECT id FROM primary_version) \
                   END \
               AND (SELECT COUNT(*) FROM linked_versions) >= 0 \
             RETURNING item.id\
         ) \
         SELECT (SELECT COUNT(*) FROM requested_items) AS requested_count, \
                (SELECT id FROM primary_version) AS primary_id, \
                (SELECT COUNT(*) FROM updated) AS updated_count",
    );
    (sql, values)
}

const BASE_ITEM_COLUMNS: &str = "id, item_type, data, path, parent_id, top_parent_id, name, \
    clean_name, sort_name, media_type, overview, official_rating, index_number, parent_index_number, production_year, \
    premiere_date, runtime_ticks, is_folder, is_virtual_item, presentation_unique_key, primary_version_id, series_id, season_id, \
    series_presentation_unique_key, date_created, date_modified, row_version";

fn scored_search_cte(
    query: &BaseItemQuery,
    clean_search_term: &str,
    clean_prefix: &str,
    like_original: &str,
) -> (String, Vec<SeaValue>) {
    let mut values = vec![
        clean_search_term.into(),
        clean_prefix.into(),
        clean_search_term.into(),
        postgres_contains_pattern(clean_search_term).into(),
        like_original.into(),
    ];
    let mut sql = String::from(
        "WITH filtered AS (\
             SELECT item.*, \
                    CASE \
                      WHEN item.clean_name = $1 THEN 100.0 \
                      WHEN item.clean_name LIKE (concat($2, '%')) THEN 80.0 \
                      WHEN item.clean_name LIKE (concat('%', $3, ' %')) THEN 75.0 \
                      ELSE 50.0 \
                    END AS search_score \
             FROM jellyfin.base_items AS item \
             WHERE item.item_type <> 'PLACEHOLDER' \
               AND item.id <> '00000000-0000-0000-0000-000000000001'::uuid \
               AND item.is_virtual_item = false \
               AND item.primary_version_id IS NULL \
               AND (item.data ->> 'OwnerId' IS NULL OR item.data ->> 'ExtraType' IS NOT NULL) \
               AND (item.clean_name ILIKE $4 OR item.data ->> 'OriginalTitle' ILIKE $5)",
    );
    append_raw_item_filters(&mut sql, &mut values, query, false);
    sql.push(')');
    (sql, values)
}

fn search_score(clean_name: Option<&str>, clean_search_term: &str, clean_prefix: &str) -> f32 {
    let Some(clean_name) = clean_name else {
        return 50.0;
    };
    if clean_name.eq_ignore_ascii_case(clean_search_term) {
        100.0
    } else if clean_name
        .get(..clean_search_term.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(clean_search_term))
    {
        80.0
    } else if clean_name.contains(clean_prefix) {
        75.0
    } else {
        50.0
    }
}

fn grouped_versions_cte(query: &BaseItemQuery) -> (String, Vec<SeaValue>) {
    let mut values = Vec::new();
    let mut sql = String::from(
        "WITH filtered AS (\
             SELECT item.* FROM jellyfin.base_items AS item \
             WHERE item.item_type <> 'PLACEHOLDER'",
    );
    append_raw_item_filters(&mut sql, &mut values, query, true);
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

fn filtered_query_cte(query: &BaseItemQuery) -> (String, Vec<SeaValue>) {
    let mut values = Vec::new();
    let mut sql = String::from(
        "WITH filtered AS (\
             SELECT item.* FROM jellyfin.base_items AS item \
             WHERE item.item_type <> 'PLACEHOLDER'",
    );
    if !query.include_alternate_versions {
        sql.push_str(" AND item.primary_version_id IS NULL");
    }
    sql.push_str(" AND (item.data ->> 'OwnerId' IS NULL OR item.data ->> 'ExtraType' IS NOT NULL)");
    append_raw_item_filters(&mut sql, &mut values, query, true);
    sql.push(')');
    (sql, values)
}

fn query_uses_advanced_filters(query: &BaseItemQuery) -> bool {
    query.max_premiere_date.is_some()
        || query.min_date_last_saved.is_some()
        || query.min_date_last_saved_for_user.is_some()
        || query.min_critic_rating.is_some()
        || query.has_overview.is_some()
        || query.has_official_rating.is_some()
        || query.has_parental_rating.is_some()
        || query.has_imdb_id.is_some()
        || query.has_tmdb_id.is_some()
        || query.has_tvdb_id.is_some()
        || query.has_subtitles.is_some()
        || query.has_theme_song.is_some()
        || query.has_theme_video.is_some()
        || query.has_special_feature.is_some()
        || query.has_trailer.is_some()
        || query.is_hd.is_some()
        || query.is_4k.is_some()
        || query.min_width.is_some()
        || query.max_width.is_some()
        || query.min_height.is_some()
        || query.max_height.is_some()
        || query.is_3d.is_some()
        || query.is_locked.is_some()
        || query.is_placeholder.is_some()
        || query.is_missing.is_some()
        || query.is_unaired.is_some()
        || query.index_number.is_some()
        || query.parent_index_number.is_some()
        || query.adjacent_to.is_some()
        || !query.location_types.is_empty()
        || !query.exclude_location_types.is_empty()
        || !query.video_types.is_empty()
        || !query.image_types.is_empty()
        || !query.series_statuses.is_empty()
        || !query.official_ratings.is_empty()
        || !query.audio_languages.is_empty()
        || !query.subtitle_languages.is_empty()
        || !query.person_ids.is_empty()
        || !query.person_types.is_empty()
        || !query.studio_ids.is_empty()
        || !query.genre_ids.is_empty()
        || !query.artist_ids.is_empty()
        || !query.exclude_artist_ids.is_empty()
        || !query.album_artist_ids.is_empty()
        || !query.contributing_artist_ids.is_empty()
        || !query.album_ids.is_empty()
        || query.name_starts_with_or_greater.is_some()
        || query.name_starts_with.is_some()
        || query.name_less_than.is_some()
        || query.collapse_box_set_items
        || !query.studios.is_empty()
        || !query.artists.is_empty()
        || !query.albums.is_empty()
}

fn resumable_filtered_cte(user_id: Uuid, query: &BaseItemQuery) -> (String, Vec<SeaValue>) {
    let mut values = vec![user_id.into()];
    let mut sql = String::from(
        "WITH progress_by_item AS MATERIALIZED (\
             SELECT item_id, MAX(last_played_date) AS resume_last_played_date \
             FROM jellyfin.user_data \
             WHERE user_id = $1 AND playback_position_ticks > 0 \
             GROUP BY item_id\
         ), progress_leaf_items AS MATERIALIZED (\
             SELECT item.id, item.primary_version_id, item.is_virtual_item, \
                    progress.resume_last_played_date \
             FROM progress_by_item AS progress \
             JOIN jellyfin.base_items AS item ON item.id = progress.item_id \
             WHERE item.is_folder = false\
         ), ranked_progress_leaf_items AS (\
             SELECT progress.*, ROW_NUMBER() OVER (\
                        PARTITION BY COALESCE(progress.primary_version_id, progress.id) \
                        ORDER BY progress.resume_last_played_date DESC NULLS LAST, progress.id\
                    ) AS resume_rank \
             FROM progress_leaf_items AS progress\
         ), in_progress_folder_ids AS (\
             SELECT closure.parent_item_id AS folder_id \
             FROM progress_leaf_items AS progress \
             JOIN jellyfin.ancestor_ids AS closure ON closure.item_id = progress.id \
             WHERE progress.is_virtual_item = false \
             UNION \
             SELECT link.parent_id AS folder_id \
             FROM progress_leaf_items AS progress \
             JOIN jellyfin.linked_children AS link ON link.child_id = progress.id \
             WHERE progress.is_virtual_item = false \
             UNION \
             SELECT link.parent_id AS folder_id \
             FROM progress_leaf_items AS progress \
             JOIN jellyfin.ancestor_ids AS closure ON closure.item_id = progress.id \
             JOIN jellyfin.linked_children AS link ON link.child_id = closure.parent_item_id \
             WHERE progress.is_virtual_item = false\
         ), played_leaf_items AS MATERIALIZED (\
             SELECT DISTINCT item.id \
             FROM jellyfin.user_data AS progress \
             JOIN jellyfin.base_items AS item ON item.id = progress.item_id \
             WHERE progress.user_id = $1 AND progress.played = true \
               AND item.is_folder = false AND item.is_virtual_item = false \
               AND item.primary_version_id IS NULL \
               AND (item.data ->> 'OwnerId' IS NULL OR item.data ->> 'ExtraType' IS NOT NULL)\
         ), played_folder_ids AS (\
             SELECT closure.parent_item_id AS folder_id \
             FROM played_leaf_items AS played \
             JOIN jellyfin.ancestor_ids AS closure ON closure.item_id = played.id \
             UNION \
             SELECT link.parent_id AS folder_id \
             FROM played_leaf_items AS played \
             JOIN jellyfin.linked_children AS link ON link.child_id = played.id \
             UNION \
             SELECT link.parent_id AS folder_id \
             FROM played_leaf_items AS played \
             JOIN jellyfin.ancestor_ids AS closure ON closure.item_id = played.id \
             JOIN jellyfin.linked_children AS link ON link.child_id = closure.parent_item_id\
         ), partial_folder_ids AS (\
             SELECT folder.id AS folder_id \
             FROM played_folder_ids AS played \
             JOIN jellyfin.base_items AS folder ON folder.id = played.folder_id \
             WHERE folder.is_folder = true AND folder.item_type IN ('Series', 'Season') AND (",
    );
    // Official Series/Season semantics require both a played and an unplayed
    // non-owned leaf. The correlated unplayed check is deliberately limited
    // to folders reached from this user's played rows instead of every item.
    append_has_descendant_leaf_condition(
        &mut sql,
        &mut values,
        user_id,
        "folder",
        false,
        LeafUserDataCondition::Unplayed,
    );
    sql.push_str(
        ")\
         ), resumable_folder_ids AS (\
             SELECT folder.id AS folder_id \
             FROM jellyfin.base_items AS folder \
             JOIN in_progress_folder_ids AS progress ON progress.folder_id = folder.id \
             WHERE folder.is_folder = true AND folder.item_type IN ('Series', 'Season') \
             UNION \
             SELECT folder_id FROM partial_folder_ids\
         ), resume_candidates AS MATERIALIZED (\
             SELECT progress.id, progress.resume_last_played_date \
             FROM ranked_progress_leaf_items AS progress \
             WHERE progress.resume_rank = 1 \
             UNION ALL \
             SELECT folder.id, progress.resume_last_played_date \
             FROM resumable_folder_ids AS resumable \
             JOIN jellyfin.base_items AS folder ON folder.id = resumable.folder_id \
             LEFT JOIN progress_by_item AS progress ON progress.item_id = folder.id\
         ), filtered AS MATERIALIZED (\
             SELECT item.*, candidate.resume_last_played_date \
             FROM resume_candidates AS candidate \
             JOIN jellyfin.base_items AS item ON item.id = candidate.id \
             WHERE item.item_type <> 'PLACEHOLDER'",
    );
    append_raw_item_filters(&mut sql, &mut values, query, true);
    sql.push(')');
    (sql, values)
}

fn not_resumable_filtered_cte(user_id: Uuid, query: &BaseItemQuery) -> (String, Vec<SeaValue>) {
    let mut values = vec![user_id.into()];
    let mut sql = String::from(
        "WITH progress_by_item AS (\
             SELECT item_id, MAX(last_played_date) AS resume_last_played_date \
             FROM jellyfin.user_data \
             WHERE user_id = $1 AND playback_position_ticks > 0 \
             GROUP BY item_id\
         ), resumable_groups AS (\
             SELECT DISTINCT COALESCE(item.primary_version_id, item.id) AS primary_id \
             FROM progress_by_item AS progress \
             JOIN jellyfin.base_items AS item ON item.id = progress.item_id \
         ), filtered AS (\
             SELECT item.* FROM jellyfin.base_items AS item \
             WHERE item.item_type <> 'PLACEHOLDER' \
               AND item.primary_version_id IS NULL \
               AND (item.data ->> 'OwnerId' IS NULL OR item.data ->> 'ExtraType' IS NOT NULL) \
               AND ((item.is_folder = true AND NOT (",
    );
    append_folder_is_resumable_condition(&mut sql, &mut values, user_id, "item");
    sql.push_str(
        ")) OR (item.is_folder = false AND NOT EXISTS (\
                   SELECT 1 FROM resumable_groups \
                   WHERE resumable_groups.primary_id = item.id\
               )))",
    );
    append_raw_item_filters(&mut sql, &mut values, query, true);
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
    append_default_owned_filter(&mut sql);
    append_raw_item_filters(&mut sql, &mut values, query, true);
    sql.push_str(
        "), dated AS (\
             SELECT item.*, version_dates.date_played \
             FROM filtered AS item \
             LEFT JOIN version_dates ON version_dates.primary_id = item.id\
         )",
    );
    (sql, values)
}

#[allow(clippy::too_many_lines, clippy::if_not_else)]
fn extended_sort_cte(query: &BaseItemQuery) -> (String, Vec<SeaValue>) {
    let mut values = Vec::new();
    let uses_item_value_sort = matches!(
        query.order,
        BaseItemOrder::ArtistAscending
            | BaseItemOrder::ArtistDescending
            | BaseItemOrder::AlbumArtistAscending
            | BaseItemOrder::AlbumArtistDescending
            | BaseItemOrder::StudioAscending
            | BaseItemOrder::StudioDescending
    );
    let uses_video_bit_rate_sort = matches!(
        query.order,
        BaseItemOrder::VideoBitRateAscending | BaseItemOrder::VideoBitRateDescending
    );
    let uses_user_data_sort = matches!(
        query.order,
        BaseItemOrder::IsUnplayedAscending
            | BaseItemOrder::IsUnplayedDescending
            | BaseItemOrder::IsPlayedAscending
            | BaseItemOrder::IsPlayedDescending
            | BaseItemOrder::IsFavoriteOrLikedAscending
            | BaseItemOrder::IsFavoriteOrLikedDescending
    );
    let uses_series_sort = matches!(
        query.order,
        BaseItemOrder::SeriesSortNameAscending | BaseItemOrder::SeriesSortNameDescending
    );
    let mut sql = String::from("WITH ");
    if uses_item_value_sort {
        sql.push_str(
            "item_value_sorts AS (\
                 SELECT map.item_id, \
                        MIN(CASE WHEN item_value.type = 0 THEN item_value.clean_value END) AS artist, \
                        MIN(CASE WHEN item_value.type = 1 THEN item_value.clean_value END) AS album_artist, \
                        MIN(CASE WHEN item_value.type = 3 THEN item_value.clean_value END) AS studio \
                 FROM jellyfin.item_value_map AS map \
                 JOIN jellyfin.item_values AS item_value \
                   ON item_value.item_value_id = map.item_value_id \
                 WHERE item_value.type IN (0, 1, 3) \
                 GROUP BY map.item_id\
             ), ",
        );
    }
    if uses_video_bit_rate_sort {
        sql.push_str(
            "video_bit_rate_sorts AS (\
                 SELECT stream.item_id, MAX(stream.bit_rate) AS video_bit_rate \
                 FROM jellyfin.media_streams AS stream \
                 JOIN jellyfin.base_items AS version ON version.id = stream.item_id \
                 WHERE stream.stream_type = 1 \
                 GROUP BY stream.item_id\
             ), ",
        );
    }
    if uses_user_data_sort {
        let Some(user_id) = query.user_id else {
            unreachable!("user-data sort queries require a user id");
        };
        values.push(user_id.into());
        let _ = write!(
            sql,
            "user_data_sorts AS (\
                 SELECT item_id, \
                        MAX((is_favorite OR likes = true)::int) AS is_favorite_or_liked, \
                        MAX(played::int) AS is_played \
                 FROM jellyfin.user_data \
                 WHERE user_id = ${} \
                 GROUP BY item_id\
             ), ",
            values.len()
        );
    }
    sql.push_str(
        "filtered AS (\
             SELECT item.*, \
                    COALESCE((
                        SELECT MAX(progress.play_count) \
                        FROM jellyfin.user_data AS progress \
                        WHERE progress.item_id = item.id \
                          AND progress.user_id = ",
    );
    if !uses_user_data_sort {
        let Some(user_id) = query.user_id else {
            unreachable!("extended-sort queries require a user id");
        };
        values.push(user_id.into());
        let _ = write!(sql, "${}", values.len());
    } else {
        sql.push_str("$1");
    }
    sql.push_str(
        "), 0)::bigint AS play_count, \
         COALESCE((item.data ->> 'CommunityRating')::double precision, 0.0) AS community_rating, \
         COALESCE((item.data ->> 'CriticRating')::double precision, 0.0) AS critic_rating,",
    );
    if uses_item_value_sort {
        sql.push_str(
            " item_value_sorts.artist AS artist, \
              item_value_sorts.album_artist AS album_artist, \
              item_value_sorts.studio AS studio,",
        );
    }
    if uses_video_bit_rate_sort {
        sql.push_str(" video_bit_rate_sorts.video_bit_rate AS video_bit_rate,");
    }
    if uses_user_data_sort {
        sql.push_str(
            " COALESCE(user_data_sorts.is_favorite_or_liked, 0)::boolean AS is_favorite_or_liked, \
             COALESCE(user_data_sorts.is_played, 0)::boolean AS is_played, \
             NOT COALESCE(user_data_sorts.is_played, 0)::boolean AS is_unplayed,",
        );
    }
    if uses_series_sort {
        sql.push_str(
            " COALESCE(NULLIF(item.data ->> 'SeriesName', ''), series.sort_name, \
              series.clean_name, series.name) AS series_sort_name,",
        );
    }
    sql.push_str(
        " NULLIF(item.data ->> 'Album', '') AS album, \
         NULLIF(item.data ->> 'StartDate', '')::timestamptz AS start_date, \
         NULLIF(item.data ->> 'DateLastMediaAdded', '')::timestamptz AS date_last_content_added, \
         CASE \
             WHEN item.item_type <> 'Episode' THEN NULL \
             WHEN COALESCE(item.parent_index_number, -1) = 0 THEN \
                 (COALESCE(NULLIF(item.data ->> 'AirsAfterSeasonNumber', '')::integer, \
                           NULLIF(item.data ->> 'AirsBeforeSeasonNumber', '')::integer, 0)::bigint \
                  * 1000000000) \
                 + CASE WHEN item.data ->> 'AirsAfterSeasonNumber' IS NOT NULL THEN 1000000 ELSE 0 END \
                 + (COALESCE(NULLIF(item.data ->> 'AirsBeforeEpisodeNumber', '')::integer, 0)::bigint * 1000) \
                 + COALESCE(item.index_number, 0) \
             ELSE ((COALESCE(item.parent_index_number, -1)::bigint * 1000) \
                   + COALESCE(item.index_number, -1)) \
         END AS aired_episode_order \
         FROM jellyfin.base_items AS item",
    );
    if uses_item_value_sort {
        sql.push_str(" LEFT JOIN item_value_sorts ON item_value_sorts.item_id = item.id");
    }
    if uses_video_bit_rate_sort {
        sql.push_str(" LEFT JOIN video_bit_rate_sorts ON video_bit_rate_sorts.item_id = item.id");
    }
    if uses_user_data_sort {
        sql.push_str(" LEFT JOIN user_data_sorts ON user_data_sorts.item_id = item.id");
    }
    if uses_series_sort {
        sql.push_str(" LEFT JOIN jellyfin.base_items AS series ON series.id = item.series_id");
    }
    sql.push_str(" WHERE item.item_type <> 'PLACEHOLDER'");
    append_default_owned_filter(&mut sql);
    append_raw_item_filters(&mut sql, &mut values, query, true);
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
    append_default_owned_filter(&mut sql);
    append_raw_item_filters(&mut sql, &mut values, query, true);
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
    append_default_owned_filter(&mut sql);
    append_raw_item_filters(&mut sql, &mut values, query, true);
    sql.push_str(
        "), ratings AS (\
             SELECT DISTINCT official_rating FROM filtered\
         )",
    );
    (sql, values)
}

#[derive(Clone, Copy)]
enum LeafUserDataCondition {
    Played,
    Unplayed,
    InProgress,
}

/// Mirrors the `TranslateQuery` default exclusion: alternate versions and
/// owned non-extra items are hidden, while extras keep their owner context.
fn append_default_owned_filter(sql: &mut String) {
    sql.push_str(
        " AND item.primary_version_id IS NULL \
           AND (item.data ->> 'OwnerId' IS NULL OR item.data ->> 'ExtraType' IS NOT NULL)",
    );
}

fn search_term_patterns(search_term: &str) -> (String, String) {
    let clean_search_term = search_term.clean_value();
    let has_wildcard = clean_search_term
        .chars()
        .any(|character| matches!(character, '%' | '_' | '[' | ']' | '^'));
    if has_wildcard {
        (
            format!("%{}%", clean_search_term.trim_matches('%')),
            format!("%{}%", search_term.trim_matches('%')),
        )
    } else {
        (format!("%{clean_search_term}%"), format!("%{search_term}%"))
    }
}

/// Builds the folder-side user-data predicate used by `IsPlayed`,
/// `IsResumable`, and their inverses.
fn append_leaf_user_data_condition(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    user_id: Uuid,
    alias: &str,
    condition: LeafUserDataCondition,
) {
    match condition {
        LeafUserDataCondition::Played => {
            push_bind(
                sql,
                values,
                user_id,
                &format!(
                    " EXISTS (SELECT 1 FROM jellyfin.user_data AS leaf_data \
                     WHERE leaf_data.item_id = {alias}.id AND leaf_data.user_id = "
                ),
            );
            sql.push_str(" AND leaf_data.played = true)");
        }
        LeafUserDataCondition::Unplayed => {
            push_bind(
                sql,
                values,
                user_id,
                &format!(
                    " NOT EXISTS (SELECT 1 FROM jellyfin.user_data AS leaf_data \
                     WHERE leaf_data.item_id = {alias}.id AND leaf_data.user_id = "
                ),
            );
            sql.push_str(" AND leaf_data.played = true)");
        }
        LeafUserDataCondition::InProgress => {
            push_bind(
                sql,
                values,
                user_id,
                &format!(
                    " EXISTS (SELECT 1 FROM jellyfin.user_data AS leaf_data \
                     WHERE leaf_data.item_id = {alias}.id AND leaf_data.user_id = "
                ),
            );
            sql.push_str(" AND leaf_data.playback_position_ticks > 0)");
        }
    }
}

#[allow(clippy::too_many_lines)]
fn append_has_descendant_leaf_condition(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    user_id: Uuid,
    table: &str,
    include_owned: bool,
    condition: LeafUserDataCondition,
) {
    let hierarchy_owned_filter = if include_owned {
        String::new()
    } else {
        " AND leaf.primary_version_id IS NULL \
           AND (leaf.data ->> 'OwnerId' IS NULL OR leaf.data ->> 'ExtraType' IS NOT NULL)"
            .to_owned()
    };
    let linked_owned_filter = if include_owned {
        String::new()
    } else {
        " AND linked.primary_version_id IS NULL \
           AND (linked.data ->> 'OwnerId' IS NULL OR linked.data ->> 'ExtraType' IS NOT NULL)"
            .to_owned()
    };

    let _ = write!(
        sql,
        "EXISTS (SELECT 1 FROM jellyfin.ancestor_ids AS closure \
         JOIN jellyfin.base_items AS leaf ON leaf.id = closure.item_id \
         WHERE closure.parent_item_id = {table}.id \
           AND leaf.is_folder = false AND leaf.is_virtual_item = false{hierarchy_owned_filter} AND "
    );
    append_leaf_user_data_condition(sql, values, user_id, "leaf", condition);
    sql.push(')');
    let _ = write!(
        sql,
        " OR EXISTS (SELECT 1 FROM jellyfin.linked_children AS link \
         JOIN jellyfin.base_items AS linked ON linked.id = link.child_id \
         WHERE link.parent_id = {table}.id AND ( \
             (linked.is_folder = false AND linked.is_virtual_item = false{linked_owned_filter} AND "
    );
    append_leaf_user_data_condition(sql, values, user_id, "linked", condition);
    sql.push(')');
    let _ = write!(
        sql,
        " OR EXISTS (SELECT 1 FROM jellyfin.ancestor_ids AS closure \
         JOIN jellyfin.base_items AS leaf ON leaf.id = closure.item_id \
         WHERE closure.parent_item_id = linked.id \
           AND leaf.is_folder = false AND leaf.is_virtual_item = false{hierarchy_owned_filter} AND "
    );
    append_leaf_user_data_condition(sql, values, user_id, "leaf", condition);
    sql.push_str(")))");
}

pub(crate) fn append_is_played_filter(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    user_id: Uuid,
    table: &str,
    is_played: bool,
) {
    sql.push_str(" (");
    let _ = write!(sql, "{table}.is_folder = false AND ");
    append_leaf_user_data_condition(
        sql,
        values,
        user_id,
        table,
        if is_played {
            LeafUserDataCondition::Played
        } else {
            LeafUserDataCondition::Unplayed
        },
    );
    let _ = write!(sql, " OR {table}.is_folder = true AND (");
    if is_played {
        sql.push_str("NOT (");
    }
    append_has_descendant_leaf_condition(
        sql,
        values,
        user_id,
        table,
        false,
        LeafUserDataCondition::Unplayed,
    );
    if is_played {
        sql.push(')');
    }
    sql.push(')');
    sql.push(')');
}

#[allow(clippy::too_many_lines)]
fn append_folder_is_resumable_condition(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    user_id: Uuid,
    table: &str,
) {
    let _ = write!(
        sql,
        "{table}.is_folder = true AND {table}.item_type IN ('Series', 'Season') AND ("
    );
    append_has_descendant_leaf_condition(
        sql,
        values,
        user_id,
        table,
        true,
        LeafUserDataCondition::InProgress,
    );
    sql.push_str(" OR (");
    append_has_descendant_leaf_condition(
        sql,
        values,
        user_id,
        table,
        false,
        LeafUserDataCondition::Played,
    );
    sql.push_str(" AND ");
    append_has_descendant_leaf_condition(
        sql,
        values,
        user_id,
        table,
        false,
        LeafUserDataCondition::Unplayed,
    );
    sql.push_str("))");
}

#[allow(clippy::too_many_lines)]
fn append_raw_item_filters(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    query: &BaseItemQuery,
    include_search_term: bool,
) {
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
    if !query.parent_ids.is_empty() {
        if query.recursive {
            sql.push_str(
                " AND item.id IN (SELECT closure.item_id \
                 FROM jellyfin.ancestor_ids AS closure WHERE 1 = 1",
            );
            append_uuid_list_filter(sql, values, "closure.parent_item_id", &query.parent_ids);
            sql.push(')');
        } else {
            append_uuid_list_filter(sql, values, "item.parent_id", &query.parent_ids);
        }
    }
    if include_search_term
        && let Some(search_term) = query
            .search_term
            .as_deref()
            .map(str::trim)
            .filter(|term| !term.is_empty())
    {
        let (clean_pattern, original_pattern) = search_term_patterns(search_term);
        push_bind(sql, values, clean_pattern, " AND (item.clean_name LIKE ");
        sql.push_str(
            " OR (item.data ->> 'OriginalTitle') IS NOT NULL \
               AND (item.data ->> 'OriginalTitle') ILIKE ",
        );
        push_bind(sql, values, original_pattern, "");
        sql.push(')');
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
    append_airing_filter(sql, query.is_airing);
    if let Some(is_virtual_item) = query.is_virtual_item {
        push_bind(sql, values, is_virtual_item, " AND item.is_virtual_item = ");
    }
    if let Some(is_played) = query.is_played {
        let Some(user_id) = query.user_id else {
            return;
        };
        sql.push_str(" AND");
        append_is_played_filter(sql, values, user_id, "item", is_played);
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
    if let Some(person) = query
        .person
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        push_bind(
            sql,
            values,
            person,
            " AND EXISTS (
                SELECT 1 FROM jellyfin.people_base_item_map AS person_map
                JOIN jellyfin.people AS person ON person.id = person_map.person_id
                WHERE person_map.item_id = item.id AND person.name = ",
        );
        if !query.person_types.is_empty() {
            sql.push_str(" AND person_map.person_type IN (");
            append_bind_list(sql, values, query.person_types.iter().cloned());
            sql.push(')');
        }
        sql.push(')');
    }
    if !query.person_ids.is_empty() {
        sql.push_str(
            " AND EXISTS (
            SELECT 1 FROM jellyfin.people_base_item_map AS person_map
            WHERE person_map.item_id = item.id
              AND person_map.person_id IN (
                  SELECT person.id
                  FROM jellyfin.base_items AS public_person
                  JOIN jellyfin.people AS person ON person.name = public_person.name
                  WHERE public_person.id IN (",
        );
        append_bind_list(sql, values, query.person_ids.iter().copied());
        sql.push_str("))");
        if !query.person_types.is_empty() {
            sql.push_str(" AND person_map.person_type IN (");
            append_bind_list(sql, values, query.person_types.iter().cloned());
            sql.push(')');
        }
        sql.push(')');
    }
    if !query.studio_ids.is_empty() {
        append_item_value_id_filter(
            sql,
            values,
            &[item_value::ItemValueType::Studios],
            &query.studio_ids,
            false,
        );
    }
    if !query.genre_ids.is_empty() {
        append_item_value_id_filter(
            sql,
            values,
            &[item_value::ItemValueType::Genre],
            &query.genre_ids,
            false,
        );
    }
    if !query.artist_ids.is_empty() {
        append_item_value_id_filter(
            sql,
            values,
            &[
                item_value::ItemValueType::Artist,
                item_value::ItemValueType::AlbumArtist,
            ],
            &query.artist_ids,
            false,
        );
    }
    if !query.exclude_artist_ids.is_empty() {
        append_item_value_id_filter(
            sql,
            values,
            &[
                item_value::ItemValueType::Artist,
                item_value::ItemValueType::AlbumArtist,
            ],
            &query.exclude_artist_ids,
            true,
        );
    }
    if !query.album_artist_ids.is_empty() {
        append_item_value_id_filter(
            sql,
            values,
            &[item_value::ItemValueType::AlbumArtist],
            &query.album_artist_ids,
            false,
        );
    }
    if !query.contributing_artist_ids.is_empty() {
        append_item_value_id_filter(
            sql,
            values,
            &[item_value::ItemValueType::Artist],
            &query.contributing_artist_ids,
            false,
        );
        append_item_value_id_filter(
            sql,
            values,
            &[item_value::ItemValueType::AlbumArtist],
            &query.contributing_artist_ids,
            true,
        );
    }
    if !query.album_ids.is_empty() {
        append_uuid_list_filter(sql, values, "item.parent_id", &query.album_ids);
    }
    if !query.studios.is_empty() {
        sql.push_str(" AND EXISTS (");
        sql.push_str(&item_value_exists_sql(
            item_value::ItemValueType::Studios,
            &query.studios,
            values,
        ));
        sql.push(')');
    }
    if !query.artists.is_empty() {
        sql.push_str(" AND (EXISTS (");
        sql.push_str(&item_value_exists_sql(
            item_value::ItemValueType::Artist,
            &query.artists,
            values,
        ));
        sql.push_str(") OR EXISTS (");
        sql.push_str(&item_value_exists_sql(
            item_value::ItemValueType::AlbumArtist,
            &query.artists,
            values,
        ));
        sql.push_str("))");
    }
    if !query.albums.is_empty() {
        sql.push_str(
            " AND item.parent_id IN (
            SELECT album.id
            FROM jellyfin.base_items AS album
            WHERE album.item_type = 'MusicAlbum'
              AND COALESCE(album.clean_name, album.name) IN (",
        );
        append_bind_list(sql, values, query.albums.iter().cloned());
        sql.push_str("))");
    }
    if let Some(index_number) = query.index_number {
        push_bind(sql, values, index_number, " AND item.index_number = ");
    }
    if let Some(parent_index_number) = query.parent_index_number {
        push_bind(
            sql,
            values,
            parent_index_number,
            " AND item.parent_index_number = ",
        );
    }
    if let Some(is_missing) = query.is_missing {
        push_bind(sql, values, is_missing, " AND item.is_virtual_item = ");
    }
    if let Some(is_unaired) = query.is_unaired {
        if is_unaired {
            sql.push_str(" AND item.premiere_date >= CURRENT_TIMESTAMP");
        } else {
            sql.push_str(" AND item.premiere_date < CURRENT_TIMESTAMP");
        }
    }
    if let Some(max_premiere_date) = query.max_premiere_date {
        push_bind(
            sql,
            values,
            max_premiere_date,
            " AND item.premiere_date <= ",
        );
    }
    if let Some(min_date_last_saved) = query.min_date_last_saved {
        push_bind(
            sql,
            values,
            min_date_last_saved,
            " AND item.date_modified >= ",
        );
    }
    if let Some(min_date_last_saved_for_user) = query.min_date_last_saved_for_user {
        push_bind(
            sql,
            values,
            min_date_last_saved_for_user,
            " AND item.date_modified >= ",
        );
    }
    if let Some(has_overview) = query.has_overview {
        if has_overview {
            sql.push_str(" AND item.overview IS NOT NULL AND btrim(item.overview) <> ''");
        } else {
            sql.push_str(" AND (item.overview IS NULL OR btrim(item.overview) = '')");
        }
    }
    if let Some(has_official_rating) = query.has_official_rating {
        if has_official_rating {
            sql.push_str(
                " AND item.official_rating IS NOT NULL AND btrim(item.official_rating) <> ''",
            );
        } else {
            sql.push_str(" AND (item.official_rating IS NULL OR btrim(item.official_rating) = '')");
        }
    }
    if let Some(has_parental_rating) = query.has_parental_rating {
        if has_parental_rating {
            sql.push_str(" AND item.official_rating IS NOT NULL");
        } else {
            sql.push_str(" AND item.official_rating IS NULL");
        }
    }
    if let Some(min_critic_rating) = query.min_critic_rating {
        push_bind(
            sql,
            values,
            min_critic_rating,
            " AND COALESCE((item.data ->> 'CriticRating')::double precision, 0.0) >= ",
        );
    }
    if !query.official_ratings.is_empty() {
        append_string_list_filter(
            sql,
            values,
            "item.official_rating",
            &query.official_ratings,
            false,
        );
    }
    if let Some(is_locked) = query.is_locked {
        push_bind(
            sql,
            values,
            is_locked,
            " AND COALESCE((item.data ->> 'IsLocked')::boolean, false) = ",
        );
    }
    if let Some(is_placeholder) = query.is_placeholder {
        push_bind(
            sql,
            values,
            is_placeholder,
            " AND COALESCE((item.data ->> 'IsPlaceHolder')::boolean, false) = ",
        );
    }
    if let Some(is_3d) = query.is_3d {
        if is_3d {
            sql.push_str(" AND item.data ? 'Video3DFormat'");
        } else {
            sql.push_str(" AND NOT (item.data ? 'Video3DFormat')");
        }
    }
    if !query.series_statuses.is_empty() {
        append_string_list_filter(
            sql,
            values,
            "item.data ->> 'Status'",
            &query.series_statuses,
            false,
        );
    }
    if !query.video_types.is_empty() {
        sql.push_str(" AND (item.data ->> 'VideoType' IN (");
        append_bind_list(sql, values, query.video_types.iter().cloned());
        sql.push_str(") OR item.data ->> 'IsoType' IN (");
        append_bind_list(sql, values, query.video_types.iter().cloned());
        sql.push_str("))");
    }
    if !query.image_types.is_empty() {
        sql.push_str(
            " AND EXISTS (
            SELECT 1 FROM jellyfin.base_item_images AS image
            WHERE image.item_id = item.id AND image.image_type IN (",
        );
        for (index, image_type) in query.image_types.iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            values.push((*image_type).into());
            let _ = write!(sql, "${}", values.len());
        }
        sql.push_str("))");
    }
    for (provider, has_provider) in [
        ("Imdb", query.has_imdb_id),
        ("Tmdb", query.has_tmdb_id),
        ("Tvdb", query.has_tvdb_id),
    ] {
        let Some(has_provider) = has_provider else {
            continue;
        };
        if has_provider {
            sql.push_str(" AND EXISTS (");
        } else {
            sql.push_str(" AND NOT EXISTS (");
        }
        sql.push_str(
            "SELECT 1 FROM jsonb_each_text(COALESCE(item.data -> 'ProviderIds', '{}'::jsonb)) \
             AS provider(provider_id, provider_value) WHERE lower(provider_id) = ",
        );
        let _ = write!(sql, "'{}'", provider.to_lowercase());
        sql.push(')');
    }
    if let Some(adjacent_to) = query.adjacent_to {
        push_bind(
            sql,
            values,
            adjacent_to,
            " AND item.parent_id = (SELECT parent_id FROM jellyfin.base_items WHERE id = ",
        );
        sql.push(')');
        push_bind(sql, values, adjacent_to, " AND item.id <> ");
    }
    if let Some(name) = query
        .name_starts_with_or_greater
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        push_bind(
            sql,
            values,
            name,
            " AND COALESCE(item.sort_name, item.clean_name, item.name) >= ",
        );
    }
    if let Some(name) = query
        .name_starts_with
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        push_bind(
            sql,
            values,
            format!("{name}%"),
            " AND COALESCE(item.sort_name, item.clean_name, item.name) LIKE ",
        );
    }
    if let Some(name) = query
        .name_less_than
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        push_bind(
            sql,
            values,
            name,
            " AND COALESCE(item.sort_name, item.clean_name, item.name) < ",
        );
    }
    if !query.location_types.is_empty() || !query.exclude_location_types.is_empty() {
        let virtual_expected = query
            .location_types
            .iter()
            .any(|value| value.eq_ignore_ascii_case("Virtual"));
        if !query.location_types.is_empty() {
            push_bind(
                sql,
                values,
                virtual_expected,
                " AND item.is_virtual_item = ",
            );
        } else if query
            .exclude_location_types
            .iter()
            .any(|value| value.eq_ignore_ascii_case("Virtual"))
        {
            push_bind(sql, values, false, " AND item.is_virtual_item = ");
        }
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
    if let Some(has_subtitles) = query.has_subtitles {
        append_leaf_or_descendant_match(
            sql,
            &stream_match_condition("{alias}", 2, None),
            has_subtitles,
        );
    }
    if !query.audio_languages.is_empty() {
        append_leaf_or_descendant_match(
            sql,
            &stream_match_condition("{alias}", 0, Some(&query.audio_languages)),
            true,
        );
    }
    if !query.subtitle_languages.is_empty() {
        append_leaf_or_descendant_match(
            sql,
            &stream_match_condition("{alias}", 2, Some(&query.subtitle_languages)),
            true,
        );
    }
    if query.is_hd.is_some() || query.is_4k.is_some() {
        let mut buckets = Vec::new();
        if query.is_hd == Some(false) {
            buckets.push(dimension_stream_condition(
                "{alias}",
                "stream.width > 0 AND stream.width < 1200",
            ));
        }
        if query.is_hd == Some(true) {
            buckets.push(dimension_stream_condition(
                "{alias}",
                "stream.width >= 1200 AND (stream.width < 3800 OR stream.height IS NULL OR stream.height < 2100)",
            ));
        }
        if query.is_4k == Some(true) {
            buckets.push(dimension_stream_condition(
                "{alias}",
                "stream.width >= 3800 OR stream.height >= 2100",
            ));
        }
        if buckets.is_empty() {
            sql.push_str(" AND 1 = 0");
        } else {
            append_leaf_or_descendant_match(sql, &buckets.join(" OR "), true);
        }
    }
    if let Some(min_width) = query.min_width {
        let value_index = values.len() + 1;
        values.push(min_width.into());
        append_leaf_or_descendant_match(
            sql,
            &dimension_stream_condition("{alias}", &format!("stream.width >= ${value_index}")),
            true,
        );
    }
    if let Some(max_width) = query.max_width {
        let value_index = values.len() + 1;
        values.push(max_width.into());
        append_leaf_or_descendant_match(
            sql,
            &format!(
                "NOT {} AND {}",
                dimension_stream_condition("{alias}", &format!("stream.width > ${value_index}")),
                dimension_stream_condition("{alias}", "stream.width IS NOT NULL")
            ),
            true,
        );
    }
    if let Some(min_height) = query.min_height {
        let value_index = values.len() + 1;
        values.push(min_height.into());
        append_leaf_or_descendant_match(
            sql,
            &dimension_stream_condition("{alias}", &format!("stream.height >= ${value_index}")),
            true,
        );
    }
    if let Some(max_height) = query.max_height {
        let value_index = values.len() + 1;
        values.push(max_height.into());
        append_leaf_or_descendant_match(
            sql,
            &format!(
                "NOT {} AND {}",
                dimension_stream_condition("{alias}", &format!("stream.height > ${value_index}")),
                dimension_stream_condition("{alias}", "stream.height IS NOT NULL")
            ),
            true,
        );
    }
    if let Some(has_theme_song) = query.has_theme_song {
        append_leaf_or_descendant_match(
            sql,
            &extra_type_condition("{alias}", "ThemeSong"),
            has_theme_song,
        );
    }
    if let Some(has_theme_video) = query.has_theme_video {
        append_leaf_or_descendant_match(
            sql,
            &extra_type_condition("{alias}", "ThemeVideo"),
            has_theme_video,
        );
    }
    if let Some(has_trailer) = query.has_trailer {
        append_leaf_or_descendant_match(
            sql,
            &extra_type_condition("{alias}", "Trailer"),
            has_trailer,
        );
    }
    if let Some(has_special_feature) = query.has_special_feature {
        append_leaf_or_descendant_match(
            sql,
            "EXISTS (\
                SELECT 1 FROM jellyfin.base_items AS extra \
                WHERE extra.data ->> 'OwnerId' = {alias}.id::text \
                  AND extra.data ->> 'ExtraType' IS NOT NULL \
                  AND extra.data ->> 'ExtraType' NOT IN ('Trailer', 'ThemeSong', 'ThemeVideo', 'Unknown')\
            )",
            has_special_feature,
        );
    }
    if let Some(condition) = policy_filter_sql("item", query) {
        sql.push_str(" AND (");
        sql.push_str(&condition);
        sql.push(')');
    }
}

fn append_leaf_or_descendant_match(sql: &mut String, condition_template: &str, expected: bool) {
    let leaf = condition_template.replace("{alias}", "item");
    let descendant = condition_template.replace("{alias}", "descendant");
    sql.push_str(" AND ");
    if !expected {
        sql.push_str("NOT ");
    }
    sql.push_str("((item.is_folder = false AND (");
    sql.push_str(&leaf);
    sql.push_str(
        ")) OR (item.is_folder = true AND EXISTS (\
            SELECT 1 FROM jellyfin.ancestor_ids AS closure \
            JOIN jellyfin.base_items AS descendant ON descendant.id = closure.item_id \
            WHERE closure.parent_item_id = item.id AND (",
    );
    sql.push_str(&descendant);
    sql.push_str("))))");
}

fn stream_match_condition(alias: &str, stream_type: i16, languages: Option<&[String]>) -> String {
    let mut condition = format!(
        "EXISTS (\
            SELECT 1 FROM jellyfin.media_streams AS stream \
            JOIN jellyfin.base_items AS version ON version.id = stream.item_id \
            WHERE COALESCE(version.primary_version_id, version.id) = {alias}.id \
              AND stream.stream_type = {stream_type}"
    );
    if let Some(languages) = languages.filter(|languages| !languages.is_empty()) {
        let values = quoted_string_list(languages);
        let has_und = languages
            .iter()
            .any(|language| language.eq_ignore_ascii_case("und"));
        if has_und {
            let _ = write!(
                condition,
                " AND (stream.language IN ({values}) OR stream.language IS NULL)"
            );
        } else {
            let _ = write!(condition, " AND stream.language IN ({values})");
        }
    }
    condition.push(')');
    condition
}

fn dimension_stream_condition(alias: &str, predicate: &str) -> String {
    format!(
        "EXISTS (\
            SELECT 1 FROM jellyfin.media_streams AS stream \
            JOIN jellyfin.base_items AS version ON version.id = stream.item_id \
            WHERE COALESCE(version.primary_version_id, version.id) = {alias}.id \
              AND stream.stream_type = 1 AND ({predicate})\
        )"
    )
}

fn extra_type_condition(alias: &str, extra_type: &str) -> String {
    format!(
        "EXISTS (\
            SELECT 1 FROM jellyfin.base_items AS extra \
            WHERE extra.data ->> 'OwnerId' = {alias}.id::text \
              AND extra.data ->> 'ExtraType' = '{extra_type}'\
        )"
    )
}

fn append_item_value_id_filter(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    value_types: &[item_value::ItemValueType],
    ids: &[Uuid],
    negated: bool,
) {
    if ids.is_empty() {
        return;
    }
    sql.push_str(" AND ");
    if negated {
        sql.push_str("NOT ");
    }
    sql.push_str(
        "EXISTS (\
            SELECT 1 FROM jellyfin.item_value_map AS value_map \
            JOIN jellyfin.item_values AS item_value \
              ON item_value.item_value_id = value_map.item_value_id \
            WHERE value_map.item_id = item.id \
              AND item_value.type IN (",
    );
    for (index, value_type) in value_types.iter().enumerate() {
        if index > 0 {
            sql.push_str(", ");
        }
        let _ = write!(sql, "{}", item_value_type_code(*value_type));
    }
    sql.push_str(") AND item_value.item_value_id IN (");
    append_bind_list(sql, values, ids.iter().copied());
    sql.push_str("))");
}

pub(crate) fn policy_filter_sql(table: &str, query: &BaseItemQuery) -> Option<String> {
    let mut parts = Vec::new();
    let collection_folder_types =
        quoted_string_list(&expand_item_type_aliases(&["CollectionFolder".to_owned()]));
    if !query.allowed_official_ratings.is_empty() {
        parts.push(format!(
            "({table}.official_rating IS NULL OR {table}.official_rating IN ({}))",
            quoted_string_list(&query.allowed_official_ratings)
        ));
    }
    if !query.allowed_parental_ratings.is_empty() {
        parts.push(format!(
            "({} OR NOT {})",
            effective_rating_matches(table, &query.allowed_parental_ratings),
            effective_rating_recognized(table)
        ));
    }
    if !query.block_unrated_items.is_empty() {
        parts.push(format!(
            "NOT ({} AND NOT {})",
            unrated_type_matches(table, &query.block_unrated_items),
            effective_rating_recognized(table)
        ));
    }
    if !query.blocked_tags.is_empty() {
        parts.push(format!(
            "NOT {}",
            inherited_tag_matches(table, &query.blocked_tags)
        ));
    }
    if !query.allowed_tags.is_empty() {
        parts.push(format!(
            "({} OR {table}.item_type = 'Person')",
            inherited_tag_matches(table, &query.allowed_tags)
        ));
    }
    if let Some(blocked) = query
        .blocked_media_folders
        .as_ref()
        .filter(|folders| !folders.is_empty())
    {
        parts.push(format!(
            "NOT (\
                ({table}.item_type IN ({collection_folder_types}) AND {table}.id IN ({})) \
                OR EXISTS (\
                    SELECT 1 FROM jellyfin.ancestor_ids AS blocked_closure \
                    JOIN jellyfin.base_items AS blocked_folder \
                      ON blocked_folder.id = blocked_closure.parent_item_id \
                    WHERE blocked_closure.item_id = {table}.id \
                      AND blocked_folder.item_type IN ({collection_folder_types}) \
                      AND blocked_folder.id IN ({})\
                )\
            )",
            quoted_uuid_list(blocked),
            quoted_uuid_list(blocked)
        ));
    }
    if !query.enable_all_folders {
        if query.enabled_folders.is_empty() {
            parts.push(format!(
                "(\
                    ({table}.item_type NOT IN ({collection_folder_types}) \
                     AND NOT EXISTS (\
                         SELECT 1 FROM jellyfin.ancestor_ids AS enabled_closure \
                         JOIN jellyfin.base_items AS enabled_folder \
                           ON enabled_folder.id = enabled_closure.parent_item_id \
                         WHERE enabled_closure.item_id = {table}.id \
                           AND enabled_folder.item_type IN ({collection_folder_types})\
                     ))\
                )"
            ));
        } else {
            parts.push(format!(
                "(\
                    ({table}.item_type NOT IN ({collection_folder_types}) \
                     AND NOT EXISTS (\
                         SELECT 1 FROM jellyfin.ancestor_ids AS enabled_closure \
                         JOIN jellyfin.base_items AS enabled_folder \
                           ON enabled_folder.id = enabled_closure.parent_item_id \
                         WHERE enabled_closure.item_id = {table}.id \
                           AND enabled_folder.item_type IN ({collection_folder_types})\
                     ))\
                    OR {table}.id IN ({})\
                    OR EXISTS (\
                        SELECT 1 FROM jellyfin.ancestor_ids AS enabled_closure \
                        JOIN jellyfin.base_items AS enabled_folder \
                          ON enabled_folder.id = enabled_closure.parent_item_id \
                        WHERE enabled_closure.item_id = {table}.id \
                          AND enabled_folder.item_type IN ({collection_folder_types}) \
                          AND enabled_folder.id IN ({})\
                    )\
                )",
                quoted_uuid_list(&query.enabled_folders),
                quoted_uuid_list(&query.enabled_folders)
            ));
        }
    }
    (!parts.is_empty()).then(|| parts.join(" AND "))
}

fn effective_rating_matches(table: &str, values: &[String]) -> String {
    let quoted = quoted_string_list(values);
    format!(
        "EXISTS (\
            SELECT 1 FROM ({}) AS effective_rating \
            WHERE effective_rating.rating IS NOT NULL \
              AND (lower(btrim(effective_rating.rating)) IN ({quoted}) \
                   OR EXISTS (\
                       SELECT 1 FROM unnest(string_to_array(effective_rating.rating, '/')) AS part \
                       WHERE lower(btrim(part)) IN ({quoted})\
                   ) \
                   OR EXISTS (\
                       SELECT 1 FROM unnest(string_to_array(effective_rating.rating, ':')) AS part \
                       WHERE lower(btrim(part)) IN ({quoted})\
                   ) \
                   OR EXISTS (\
                       SELECT 1 FROM unnest(string_to_array(effective_rating.rating, ' ')) AS part \
                       WHERE lower(btrim(part)) IN ({quoted})\
                   ))\
        )",
        effective_rating_values(table)
    )
}

fn effective_rating_recognized(table: &str) -> String {
    format!(
        "EXISTS (\
            SELECT 1 FROM ({}) AS effective_rating \
            WHERE effective_rating.rating IS NOT NULL \
              AND btrim(effective_rating.rating) <> '' \
              AND lower(btrim(effective_rating.rating)) NOT IN ('n/a', 'unrated', 'not rated', 'nr')\
        )",
        effective_rating_values(table)
    )
}

fn effective_rating_values(table: &str) -> String {
    format!(
        "SELECT COALESCE(\
            (SELECT custom_ratings.rating FROM (\
                SELECT 0 AS priority, \
                       NULLIF(btrim({table}.data ->> 'CustomRating'), '') AS rating \
                UNION ALL \
                SELECT 1, NULLIF(btrim(series.data ->> 'CustomRating'), '') \
                  FROM jellyfin.base_items AS series WHERE series.id = {table}.series_id \
                UNION ALL \
                SELECT 2 + closure.depth, \
                       NULLIF(btrim(ancestor.data ->> 'CustomRating'), '') \
                  FROM jellyfin.ancestor_ids AS closure \
                  JOIN jellyfin.base_items AS ancestor \
                    ON ancestor.id = closure.parent_item_id \
                  WHERE closure.item_id = {table}.id \
                UNION ALL \
                SELECT 100, NULLIF(btrim(top_parent.data ->> 'CustomRating'), '') \
                  FROM jellyfin.base_items AS top_parent \
                  WHERE top_parent.id = {table}.top_parent_id\
            ) AS custom_ratings \
            WHERE custom_ratings.rating IS NOT NULL \
            ORDER BY custom_ratings.priority, custom_ratings.rating \
            LIMIT 1), \
            (SELECT official_ratings.rating FROM (\
                SELECT 0 AS priority, \
                       NULLIF(btrim({table}.official_rating), '') AS rating \
                UNION ALL \
                SELECT 1, NULLIF(btrim(series.official_rating), '') \
                  FROM jellyfin.base_items AS series WHERE series.id = {table}.series_id \
                UNION ALL \
                SELECT 2 + closure.depth, \
                       NULLIF(btrim(ancestor.official_rating), '') \
                  FROM jellyfin.ancestor_ids AS closure \
                  JOIN jellyfin.base_items AS ancestor \
                    ON ancestor.id = closure.parent_item_id \
                  WHERE closure.item_id = {table}.id \
                UNION ALL \
                SELECT 100, NULLIF(btrim(top_parent.official_rating), '') \
                  FROM jellyfin.base_items AS top_parent \
                  WHERE top_parent.id = {table}.top_parent_id\
            ) AS official_ratings \
            WHERE official_ratings.rating IS NOT NULL \
            ORDER BY official_ratings.priority, official_ratings.rating \
            LIMIT 1)\
        ) AS rating"
    )
}

fn unrated_type_matches(table: &str, block_unrated_items: &[String]) -> String {
    let values = quoted_string_list(block_unrated_items);
    format!(
        "(\
            ({table}.item_type IN ('Movie') AND 'Movie' IN ({values})) \
            OR ({table}.item_type IN ('Trailer') AND 'Trailer' IN ({values})) \
            OR ({table}.item_type IN ('Series', 'Season', 'Episode') AND 'Series' IN ({values})) \
            OR ({table}.item_type IN ('MusicAlbum', 'MusicArtist', 'Audio', 'MusicVideo') AND 'Music' IN ({values})) \
            OR ({table}.item_type IN ('Book', 'AudioBook') AND 'Book' IN ({values})) \
            OR ({table}.item_type IN ('LiveTvChannel') AND 'LiveTvChannel' IN ({values})) \
            OR ({table}.item_type IN ('LiveTvProgram', 'Program') AND 'LiveTvProgram' IN ({values})) \
            OR ({table}.data ->> 'ChannelId' IS NOT NULL \
                AND {table}.item_type NOT IN ('LiveTvChannel', 'LiveTvProgram', 'Program') \
                AND 'ChannelContent' IN ({values}))\
        )"
    )
}

fn inherited_tag_matches(table: &str, values: &[String]) -> String {
    format!(
        "EXISTS (\
            SELECT 1 FROM jellyfin.item_value_map AS inherited_map \
            JOIN jellyfin.item_values AS inherited_value \
              ON inherited_value.item_value_id = inherited_map.item_value_id \
            WHERE inherited_value.type = {} \
              AND inherited_value.clean_value IN ({}) \
              AND (inherited_map.item_id = {table}.id \
                   OR ({table}.series_id IS NOT NULL AND inherited_map.item_id = {table}.series_id) \
                   OR EXISTS (\
                       SELECT 1 FROM jellyfin.ancestor_ids AS inherited_closure \
                       WHERE inherited_closure.item_id = {table}.id \
                         AND inherited_closure.parent_item_id = inherited_map.item_id\
                   ) \
                   OR ({table}.top_parent_id IS NOT NULL AND inherited_map.item_id = {table}.top_parent_id))\
        )",
        item_value_type_code(item_value::ItemValueType::Tags),
        quoted_string_list(values)
    )
}

fn quoted_string_list(values: &[String]) -> String {
    values
        .iter()
        .map(|value| format!("'{}'", value.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ")
}

fn quoted_uuid_list(values: &[Uuid]) -> String {
    values
        .iter()
        .map(|id| format!("'{}'", id.simple()))
        .collect::<Vec<_>>()
        .join(", ")
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

fn append_airing_filter(sql: &mut String, expected: Option<bool>) {
    let Some(expected) = expected else {
        return;
    };
    sql.push_str(" AND (");
    sql.push_str(&airing_expression("item.", expected));
    sql.push(')');
}

fn airing_expression(prefix: &str, expected: bool) -> String {
    let start_date = format!("NULLIF({prefix}data ->> 'StartDate', '')::timestamptz");
    let end_date = format!("NULLIF({prefix}data ->> 'EndDate', '')::timestamptz");
    if expected {
        format!("{start_date} <= CURRENT_TIMESTAMP AND {end_date} >= CURRENT_TIMESTAMP")
    } else {
        format!("{start_date} > CURRENT_TIMESTAMP OR {end_date} < CURRENT_TIMESTAMP")
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

#[cfg(test)]
mod tests {
    use sea_orm::Value as SeaValue;

    use super::{BaseItemQuery, append_raw_item_filters};

    #[test]
    fn raw_item_filters_expand_canonical_and_persisted_type_names() {
        let query = BaseItemQuery {
            include_item_types: vec!["movie".to_owned()],
            exclude_item_types: vec!["MediaBrowser.Controller.Entities.TV.Episode".to_owned()],
            ..Default::default()
        };
        let mut sql = String::new();
        let mut values = Vec::new();

        append_raw_item_filters(&mut sql, &mut values, &query, false);

        assert!(sql.contains("item.item_type IN ($1, $2)"));
        assert!(sql.contains("item.item_type NOT IN ($3, $4)"));
        assert_eq!(
            string_values(&values),
            [
                "Movie",
                "MediaBrowser.Controller.Entities.Movies.Movie",
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
