use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, DbBackend, DbErr, EntityTrait, FromQueryResult, Order, QueryFilter, QueryOrder,
    QuerySelect, Set, Statement, sea_query::OnConflict,
};
use std::collections::HashMap;
use thiserror::Error;
use uuid::Uuid;

use crate::entities::user_data;

/// Complete values used for an atomic user-data upsert.
#[derive(Debug, Clone, PartialEq)]
pub struct NewUserData {
    pub item_id: Uuid,
    pub user_id: Uuid,
    pub custom_data_key: String,
    pub rating: Option<f64>,
    pub playback_position_ticks: i64,
    pub play_count: i32,
    pub is_favorite: bool,
    pub last_played_date: Option<DateTime<Utc>>,
    pub played: bool,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
    pub likes: Option<bool>,
    pub retention_date: Option<DateTime<Utc>>,
}

impl NewUserData {
    #[must_use]
    pub fn new(item_id: Uuid, user_id: Uuid, custom_data_key: impl Into<String>) -> Self {
        Self {
            item_id,
            user_id,
            custom_data_key: custom_data_key.into(),
            rating: None,
            playback_position_ticks: 0,
            play_count: 0,
            is_favorite: false,
            last_played_date: None,
            played: false,
            audio_stream_index: None,
            subtitle_stream_index: None,
            likes: None,
            retention_date: None,
        }
    }
}

/// Optional field updates. Nested options allow nullable fields to be cleared.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UserDataPatch {
    pub rating: Option<Option<f64>>,
    pub playback_position_ticks: Option<i64>,
    pub play_count: Option<i32>,
    pub is_favorite: Option<bool>,
    pub last_played_date: Option<Option<DateTime<Utc>>>,
    pub played: Option<bool>,
    pub audio_stream_index: Option<Option<i32>>,
    pub subtitle_stream_index: Option<Option<i32>>,
    pub likes: Option<Option<bool>>,
    pub retention_date: Option<Option<DateTime<Utc>>>,
}

/// Fields supported by the generic user-data API update.
///
/// Unlike [`UserDataPatch`], `None` means no update for nullable fields too,
/// matching the API contract where missing and explicit JSON null are both
/// ignored.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GenericUserDataPatch {
    pub rating: Option<f64>,
    pub playback_position_ticks: Option<i64>,
    pub play_count: Option<i32>,
    pub is_favorite: Option<bool>,
    pub likes: Option<bool>,
    pub last_played_date: Option<DateTime<Utc>>,
    pub played: Option<bool>,
}

/// Common filters used for played, favorite, resume, and recent-item queries.
#[derive(Debug, Clone, Default)]
pub struct UserDataQuery {
    pub user_id: Uuid,
    pub item_ids: Vec<Uuid>,
    pub played: Option<bool>,
    pub is_favorite: Option<bool>,
    pub has_playback_position: Option<bool>,
    pub min_last_played_date: Option<DateTime<Utc>>,
    pub max_last_played_date: Option<DateTime<Utc>>,
    pub order_by_last_played_desc: bool,
    pub limit: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreferredUserDataKey {
    pub item_id: Uuid,
    pub custom_data_key: String,
    pub priority: i32,
}

impl PreferredUserDataKey {
    #[must_use]
    pub fn new(item_id: Uuid, custom_data_key: impl Into<String>, priority: i32) -> Self {
        Self {
            item_id,
            custom_data_key: custom_data_key.into(),
            priority,
        }
    }
}

#[derive(Debug, Error)]
pub enum UserDataError {
    #[error("at least one user-data key is required")]
    EmptyKey,
    #[error("rating must be between 0 and 10")]
    InvalidRating,
    #[error("playback position and play count cannot be negative")]
    NegativePlaybackValue,
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// PostgreSQL-backed per-user media state.
#[derive(Clone)]
pub struct UserDataRepository {
    database: crate::SharedDatabase,
}

impl UserDataRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Resolves a user-data row using current keys in priority order, then the
    /// lexically first retained key, in one `PostgreSQL` query.
    ///
    /// # Errors
    ///
    /// Returns a validation error when no current key is supplied, or a
    /// database error when lookup fails.
    pub async fn resolve_preferred(
        &self,
        item_id: Uuid,
        user_id: Uuid,
        keys: &[String],
    ) -> Result<Option<user_data::Model>, UserDataError> {
        if keys.is_empty() {
            return Err(UserDataError::EmptyKey);
        }
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            WITH preferred_keys AS (
                SELECT value AS custom_data_key, ordinality AS priority
                FROM jsonb_array_elements_text($3::jsonb) WITH ORDINALITY
            )
            SELECT data.item_id, data.user_id, data.custom_data_key, data.rating,
                data.playback_position_ticks, data.play_count, data.is_favorite,
                data.last_played_date, data.played, data.audio_stream_index,
                data.subtitle_stream_index, data.likes, data.retention_date
            FROM jellyfin.user_data AS data
            LEFT JOIN preferred_keys AS preferred USING (custom_data_key)
            WHERE data.item_id = $1 AND data.user_id = $2
            ORDER BY preferred.priority NULLS LAST, data.custom_data_key
            LIMIT 1
            ",
            [
                item_id.into(),
                user_id.into(),
                serde_json::json!(keys).into(),
            ],
        );
        Ok(user_data::Model::find_by_statement(statement)
            .one(self.database.as_ref())
            .await?)
    }

    /// Resolves preferred user-data rows for many items in one `PostgreSQL`
    /// query.
    ///
    /// Current keys are ranked by caller-provided priority. When none match,
    /// the lexically first retained key for that item is returned, matching
    /// [`Self::resolve_preferred`].
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn resolve_preferred_for_items(
        &self,
        user_id: Uuid,
        keys: &[PreferredUserDataKey],
    ) -> Result<HashMap<Uuid, user_data::Model>, UserDataError> {
        if keys.is_empty() {
            return Ok(HashMap::new());
        }

        let keys = keys
            .iter()
            .map(|key| {
                serde_json::json!({
                    "item_id": key.item_id.to_string(),
                    "custom_data_key": key.custom_data_key,
                    "priority": key.priority,
                })
            })
            .collect::<Vec<_>>();
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            WITH preferred_keys AS (
                SELECT item_id, custom_data_key, priority
                FROM jsonb_to_recordset($2::jsonb)
                    AS keys(item_id uuid, custom_data_key text, priority integer)
            ), requested_items AS (
                SELECT DISTINCT item_id
                FROM preferred_keys
            )
            SELECT DISTINCT ON (data.item_id)
                data.item_id, data.user_id, data.custom_data_key, data.rating,
                data.playback_position_ticks, data.play_count, data.is_favorite,
                data.last_played_date, data.played, data.audio_stream_index,
                data.subtitle_stream_index, data.likes, data.retention_date
            FROM jellyfin.user_data AS data
            INNER JOIN requested_items AS requested
                ON requested.item_id = data.item_id
            LEFT JOIN preferred_keys AS preferred
                ON preferred.item_id = data.item_id
                AND preferred.custom_data_key = data.custom_data_key
            WHERE data.user_id = $1
            ORDER BY data.item_id, preferred.priority NULLS LAST, data.custom_data_key
            ",
            [user_id.into(), serde_json::json!(keys).into()],
        );
        let rows = user_data::Model::find_by_statement(statement)
            .all(self.database.as_ref())
            .await?;
        Ok(rows.into_iter().map(|row| (row.item_id, row)).collect())
    }

    /// Atomically applies the generic API patch to a preferred current or
    /// retained user-data key and returns the resulting row.
    ///
    /// Rating wins when rating and likes are both present. A rating derives
    /// likes using Jellyfin's `6.5` threshold; a standalone likes value stores
    /// rating `10` or `1`. Columns absent from the patch retain their values.
    ///
    /// # Errors
    ///
    /// Returns a validation error for invalid numeric values or an empty key
    /// list, or a database error when the upsert fails.
    #[allow(
        clippy::too_many_lines,
        reason = "the PostgreSQL CTE and conflict update must remain one atomic statement"
    )]
    pub async fn apply_generic_patch(
        &self,
        item_id: Uuid,
        user_id: Uuid,
        keys: &[String],
        patch: GenericUserDataPatch,
    ) -> Result<user_data::Model, UserDataError> {
        let primary_key = keys.first().ok_or(UserDataError::EmptyKey)?;
        validate_optional_values(
            patch.rating,
            patch.playback_position_ticks,
            patch.play_count,
        )?;

        let rating_present = patch.rating.is_some();
        let position_present = patch.playback_position_ticks.is_some();
        let count_present = patch.play_count.is_some();
        let favorite_present = patch.is_favorite.is_some();
        let last_played_present = patch.last_played_date.is_some();
        let played_present = patch.played.is_some();
        let likes_present = patch.likes.is_some();
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            WITH preferred_keys AS (
                SELECT value AS custom_data_key, ordinality AS priority
                FROM jsonb_array_elements_text($3::jsonb) WITH ORDINALITY
            ), chosen_key AS (
                SELECT data.custom_data_key
                FROM jellyfin.user_data AS data
                LEFT JOIN preferred_keys AS preferred USING (custom_data_key)
                WHERE data.item_id = $1 AND data.user_id = $2
                ORDER BY preferred.priority NULLS LAST, data.custom_data_key
                LIMIT 1
            ), target_key AS (
                SELECT COALESCE(
                    (SELECT custom_data_key FROM chosen_key),
                    $4::text
                ) AS custom_data_key
            )
            INSERT INTO jellyfin.user_data (
                item_id, user_id, custom_data_key, rating,
                playback_position_ticks, play_count, is_favorite,
                last_played_date, played, likes
            )
            SELECT $1, $2, custom_data_key,
                CASE
                    WHEN $5::boolean THEN $6::double precision
                    WHEN $17::boolean THEN CASE WHEN $18::boolean THEN 10.0 ELSE 1.0 END
                    ELSE NULL
                END,
                CASE WHEN $7::boolean THEN $8::bigint ELSE 0 END,
                CASE WHEN $9::boolean THEN $10::integer ELSE 0 END,
                CASE WHEN $11::boolean THEN $12::boolean ELSE false END,
                CASE WHEN $13::boolean THEN $14::timestamptz ELSE NULL END,
                CASE WHEN $15::boolean THEN $16::boolean ELSE false END,
                CASE
                    WHEN $5::boolean THEN $6::double precision >= 6.5
                    WHEN $17::boolean THEN $18::boolean
                    ELSE NULL
                END
            FROM target_key
            ON CONFLICT (item_id, user_id, custom_data_key) DO UPDATE
            SET rating = CASE
                    WHEN $5::boolean THEN $6::double precision
                    WHEN $17::boolean THEN CASE WHEN $18::boolean THEN 10.0 ELSE 1.0 END
                    ELSE jellyfin.user_data.rating
                END,
                playback_position_ticks = CASE
                    WHEN $7::boolean THEN $8::bigint
                    ELSE jellyfin.user_data.playback_position_ticks
                END,
                play_count = CASE
                    WHEN $9::boolean THEN $10::integer
                    ELSE jellyfin.user_data.play_count
                END,
                is_favorite = CASE
                    WHEN $11::boolean THEN $12::boolean
                    ELSE jellyfin.user_data.is_favorite
                END,
                last_played_date = CASE
                    WHEN $13::boolean THEN $14::timestamptz
                    ELSE jellyfin.user_data.last_played_date
                END,
                played = CASE
                    WHEN $15::boolean THEN $16::boolean
                    ELSE jellyfin.user_data.played
                END,
                likes = CASE
                    WHEN $5::boolean THEN $6::double precision >= 6.5
                    WHEN $17::boolean THEN $18::boolean
                    WHEN jellyfin.user_data.rating IS NOT NULL
                        THEN jellyfin.user_data.rating >= 6.5
                    ELSE NULL
                END
            RETURNING item_id, user_id, custom_data_key, rating,
                playback_position_ticks, play_count, is_favorite,
                last_played_date, played, audio_stream_index,
                subtitle_stream_index, likes, retention_date
            ",
            [
                item_id.into(),
                user_id.into(),
                serde_json::json!(keys).into(),
                primary_key.as_str().into(),
                rating_present.into(),
                patch.rating.unwrap_or_default().into(),
                position_present.into(),
                patch.playback_position_ticks.unwrap_or_default().into(),
                count_present.into(),
                patch.play_count.unwrap_or_default().into(),
                favorite_present.into(),
                patch.is_favorite.unwrap_or_default().into(),
                last_played_present.into(),
                patch.last_played_date.into(),
                played_present.into(),
                patch.played.unwrap_or_default().into(),
                likes_present.into(),
                patch.likes.unwrap_or_default().into(),
            ],
        );
        user_data::Model::find_by_statement(statement)
            .one(self.database.as_ref())
            .await?
            .ok_or_else(|| {
                DbErr::RecordNotFound("generic user-data upsert returned no row".to_owned()).into()
            })
    }

    /// Atomically changes only the favorite flag for one user-data row.
    ///
    /// `PostgreSQL`'s conflict update deliberately names no other columns, so a
    /// concurrent playstate, rating, or stream-selection write is preserved.
    ///
    /// # Errors
    ///
    /// Returns a database error when the upsert fails.
    pub async fn set_favorite(
        &self,
        item_id: Uuid,
        user_id: Uuid,
        keys: &[String],
        is_favorite: bool,
    ) -> Result<user_data::Model, UserDataError> {
        let primary_key = keys.first().ok_or(UserDataError::EmptyKey)?;
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            WITH preferred_keys AS (
                SELECT value AS custom_data_key, ordinality AS priority
                FROM jsonb_array_elements_text($3::jsonb) WITH ORDINALITY
            ), chosen_key AS (
                SELECT data.custom_data_key
                FROM jellyfin.user_data AS data
                LEFT JOIN preferred_keys AS preferred
                    USING (custom_data_key)
                WHERE data.item_id = $1 AND data.user_id = $2
                ORDER BY preferred.priority NULLS LAST, data.custom_data_key
                LIMIT 1
            ), target_key AS (
                SELECT COALESCE(
                    (SELECT custom_data_key FROM chosen_key),
                    $4::text
                ) AS custom_data_key
            )
            INSERT INTO jellyfin.user_data (
                item_id, user_id, custom_data_key, is_favorite
            )
            SELECT $1, $2, custom_data_key, $5
            FROM target_key
            ON CONFLICT (item_id, user_id, custom_data_key) DO UPDATE
            SET is_favorite = EXCLUDED.is_favorite
            RETURNING item_id, user_id, custom_data_key, rating,
                playback_position_ticks, play_count, is_favorite,
                last_played_date, played, audio_stream_index,
                subtitle_stream_index, likes, retention_date
            ",
            [
                item_id.into(),
                user_id.into(),
                serde_json::json!(keys).into(),
                primary_key.as_str().into(),
                is_favorite.into(),
            ],
        );
        user_data::Model::find_by_statement(statement)
            .one(self.database.as_ref())
            .await?
            .ok_or_else(|| {
                DbErr::RecordNotFound("favorite upsert returned no row".to_owned()).into()
            })
    }

    /// Atomically changes only the boolean rating columns for one user-data row.
    ///
    /// `true` stores Jellyfin's like rating `(10, true)`, `false` stores its
    /// dislike rating `(1, false)`, and `None` clears both columns. Concurrent
    /// favorite, playstate, and stream-selection writes are preserved.
    ///
    /// # Errors
    ///
    /// Returns a validation error when no current key is supplied, or a
    /// database error when the upsert fails.
    pub async fn set_rating(
        &self,
        item_id: Uuid,
        user_id: Uuid,
        keys: &[String],
        likes: Option<bool>,
    ) -> Result<user_data::Model, UserDataError> {
        let primary_key = keys.first().ok_or(UserDataError::EmptyKey)?;
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            WITH preferred_keys AS (
                SELECT value AS custom_data_key, ordinality AS priority
                FROM jsonb_array_elements_text($3::jsonb) WITH ORDINALITY
            ), chosen_key AS (
                SELECT data.custom_data_key
                FROM jellyfin.user_data AS data
                LEFT JOIN preferred_keys AS preferred
                    USING (custom_data_key)
                WHERE data.item_id = $1 AND data.user_id = $2
                ORDER BY preferred.priority NULLS LAST, data.custom_data_key
                LIMIT 1
            ), target_key AS (
                SELECT COALESCE(
                    (SELECT custom_data_key FROM chosen_key),
                    $4::text
                ) AS custom_data_key
            )
            INSERT INTO jellyfin.user_data (
                item_id, user_id, custom_data_key, rating, likes
            )
            SELECT $1, $2, custom_data_key,
                CASE $5::boolean
                    WHEN true THEN 10.0
                    WHEN false THEN 1.0
                    ELSE NULL
                END,
                $5::boolean
            FROM target_key
            ON CONFLICT (item_id, user_id, custom_data_key) DO UPDATE
            SET rating = EXCLUDED.rating,
                likes = EXCLUDED.likes
            RETURNING item_id, user_id, custom_data_key, rating,
                playback_position_ticks, play_count, is_favorite,
                last_played_date, played, audio_stream_index,
                subtitle_stream_index, likes, retention_date
            ",
            [
                item_id.into(),
                user_id.into(),
                serde_json::json!(keys).into(),
                primary_key.as_str().into(),
                likes.into(),
            ],
        );
        user_data::Model::find_by_statement(statement)
            .one(self.database.as_ref())
            .await?
            .ok_or_else(|| DbErr::RecordNotFound("rating upsert returned no row".to_owned()).into())
    }

    /// Atomically applies playback progress and remembered stream-selection changes.
    ///
    /// `PostgreSQL` resolves the preferred custom-data key and upserts the
    /// mutable playback fields in one statement so concurrent favorite/rating
    /// writes are preserved.
    ///
    /// # Errors
    ///
    /// Returns a validation error when no key is supplied or numeric playback
    /// values are invalid, or a database error when the upsert fails.
    pub async fn apply_playback_progress_patch(
        &self,
        item_id: Uuid,
        user_id: Uuid,
        keys: &[String],
        patch: UserDataPatch,
    ) -> Result<user_data::Model, UserDataError> {
        let primary_key = keys.first().ok_or(UserDataError::EmptyKey)?;
        validate_optional_values(None, patch.playback_position_ticks, None)?;

        let position_present = patch.playback_position_ticks.is_some();
        let audio_present = patch.audio_stream_index.is_some();
        let subtitle_present = patch.subtitle_stream_index.is_some();
        let played_present = patch.played.is_some();
        let audio_stream_index = patch.audio_stream_index.flatten();
        let subtitle_stream_index = patch.subtitle_stream_index.flatten();
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            WITH preferred_keys AS (
                SELECT value AS custom_data_key, ordinality AS priority
                FROM jsonb_array_elements_text($3::jsonb) WITH ORDINALITY
            ), chosen_key AS (
                SELECT data.custom_data_key
                FROM jellyfin.user_data AS data
                LEFT JOIN preferred_keys AS preferred
                    USING (custom_data_key)
                WHERE data.item_id = $1 AND data.user_id = $2
                ORDER BY preferred.priority NULLS LAST, data.custom_data_key
                LIMIT 1
            ), target_key AS (
                SELECT COALESCE(
                    (SELECT custom_data_key FROM chosen_key),
                    $4::text
                ) AS custom_data_key
            )
            INSERT INTO jellyfin.user_data (
                item_id, user_id, custom_data_key,
                playback_position_ticks, audio_stream_index,
                subtitle_stream_index, played
            )
            SELECT $1, $2, custom_data_key,
                CASE WHEN $5::boolean THEN $6::bigint ELSE 0 END,
                CASE WHEN $7::boolean THEN $8::integer ELSE NULL END,
                CASE WHEN $9::boolean THEN $10::integer ELSE NULL END,
                CASE WHEN $11::boolean THEN $12::boolean ELSE false END
            FROM target_key
            ON CONFLICT (item_id, user_id, custom_data_key) DO UPDATE
            SET playback_position_ticks = CASE
                    WHEN $5::boolean THEN $6::bigint
                    ELSE jellyfin.user_data.playback_position_ticks
                END,
                audio_stream_index = CASE
                    WHEN $7::boolean THEN $8::integer
                    ELSE jellyfin.user_data.audio_stream_index
                END,
                subtitle_stream_index = CASE
                    WHEN $9::boolean THEN $10::integer
                    ELSE jellyfin.user_data.subtitle_stream_index
                END,
                played = CASE
                    WHEN $11::boolean THEN $12::boolean
                    ELSE jellyfin.user_data.played
                END
            RETURNING item_id, user_id, custom_data_key, rating,
                playback_position_ticks, play_count, is_favorite,
                last_played_date, played, audio_stream_index,
                subtitle_stream_index, likes, retention_date
            ",
            [
                item_id.into(),
                user_id.into(),
                serde_json::json!(keys).into(),
                primary_key.as_str().into(),
                position_present.into(),
                patch.playback_position_ticks.unwrap_or_default().into(),
                audio_present.into(),
                audio_stream_index.into(),
                subtitle_present.into(),
                subtitle_stream_index.into(),
                played_present.into(),
                patch.played.unwrap_or_default().into(),
            ],
        );
        user_data::Model::find_by_statement(statement)
            .one(self.database.as_ref())
            .await?
            .ok_or_else(|| {
                DbErr::RecordNotFound("playback progress upsert returned no row".to_owned()).into()
            })
    }

    /// Atomically records a playback start.
    ///
    /// `PostgreSQL` resolves the preferred user-data key, creates a row when
    /// needed, increments `play_count`, and updates `last_played_date` without
    /// disturbing resume, favorite, rating, or stream-selection state.
    ///
    /// # Errors
    ///
    /// Returns a validation error when no key is supplied, or a database error
    /// when the upsert fails.
    pub async fn record_playback_start(
        &self,
        item_id: Uuid,
        user_id: Uuid,
        keys: &[String],
        date_played: DateTime<Utc>,
        mark_played: bool,
    ) -> Result<user_data::Model, UserDataError> {
        let primary_key = keys.first().ok_or(UserDataError::EmptyKey)?;
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            WITH preferred_keys AS (
                SELECT value AS custom_data_key, ordinality AS priority
                FROM jsonb_array_elements_text($3::jsonb) WITH ORDINALITY
            ), chosen_key AS (
                SELECT data.custom_data_key
                FROM jellyfin.user_data AS data
                LEFT JOIN preferred_keys AS preferred
                    USING (custom_data_key)
                WHERE data.item_id = $1 AND data.user_id = $2
                ORDER BY preferred.priority NULLS LAST, data.custom_data_key
                LIMIT 1
            ), target_key AS (
                SELECT COALESCE(
                    (SELECT custom_data_key FROM chosen_key),
                    $4::text
                ) AS custom_data_key
            )
            INSERT INTO jellyfin.user_data (
                item_id, user_id, custom_data_key,
                play_count, last_played_date, played
            )
            SELECT $1, $2, custom_data_key, 1, $5, $6
            FROM target_key
            ON CONFLICT (item_id, user_id, custom_data_key) DO UPDATE
            SET play_count = jellyfin.user_data.play_count + 1,
                last_played_date = EXCLUDED.last_played_date,
                played = CASE
                    WHEN $6::boolean THEN true
                    ELSE jellyfin.user_data.played
                END
            RETURNING item_id, user_id, custom_data_key, rating,
                playback_position_ticks, play_count, is_favorite,
                last_played_date, played, audio_stream_index,
                subtitle_stream_index, likes, retention_date
            ",
            [
                item_id.into(),
                user_id.into(),
                serde_json::json!(keys).into(),
                primary_key.as_str().into(),
                date_played.into(),
                mark_played.into(),
            ],
        );
        user_data::Model::find_by_statement(statement)
            .one(self.database.as_ref())
            .await?
            .ok_or_else(|| {
                DbErr::RecordNotFound("playback start upsert returned no row".to_owned()).into()
            })
    }

    /// Atomically records a playback stop.
    ///
    /// `PostgreSQL` resolves the preferred user-data key, writes the computed
    /// resume position, optionally updates the played flag, and optionally
    /// increments `play_count` for clients that could not report a stop
    /// position. Unrelated rating, favorite, date, and stream-selection state
    /// is preserved.
    ///
    /// # Errors
    ///
    /// Returns a validation error when no key is supplied or the computed
    /// playback position is negative, or a database error when the upsert fails.
    pub async fn record_playback_stop(
        &self,
        item_id: Uuid,
        user_id: Uuid,
        keys: &[String],
        playback_position_ticks: i64,
        played: Option<bool>,
        increment_play_count: bool,
    ) -> Result<user_data::Model, UserDataError> {
        let primary_key = keys.first().ok_or(UserDataError::EmptyKey)?;
        validate_optional_values(None, Some(playback_position_ticks), None)?;

        let played_present = played.is_some();
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            WITH preferred_keys AS (
                SELECT value AS custom_data_key, ordinality AS priority
                FROM jsonb_array_elements_text($3::jsonb) WITH ORDINALITY
            ), chosen_key AS (
                SELECT data.custom_data_key
                FROM jellyfin.user_data AS data
                LEFT JOIN preferred_keys AS preferred
                    USING (custom_data_key)
                WHERE data.item_id = $1 AND data.user_id = $2
                ORDER BY preferred.priority NULLS LAST, data.custom_data_key
                LIMIT 1
            ), target_key AS (
                SELECT COALESCE(
                    (SELECT custom_data_key FROM chosen_key),
                    $4::text
                ) AS custom_data_key
            )
            INSERT INTO jellyfin.user_data (
                item_id, user_id, custom_data_key,
                playback_position_ticks, play_count, played
            )
            SELECT $1, $2, custom_data_key,
                $5,
                CASE WHEN $8::boolean THEN 1 ELSE 0 END,
                CASE WHEN $6::boolean THEN $7::boolean ELSE false END
            FROM target_key
            ON CONFLICT (item_id, user_id, custom_data_key) DO UPDATE
            SET playback_position_ticks = EXCLUDED.playback_position_ticks,
                play_count = CASE
                    WHEN $8::boolean THEN jellyfin.user_data.play_count + 1
                    ELSE jellyfin.user_data.play_count
                END,
                played = CASE
                    WHEN $6::boolean THEN $7::boolean
                    ELSE jellyfin.user_data.played
                END
            RETURNING item_id, user_id, custom_data_key, rating,
                playback_position_ticks, play_count, is_favorite,
                last_played_date, played, audio_stream_index,
                subtitle_stream_index, likes, retention_date
            ",
            [
                item_id.into(),
                user_id.into(),
                serde_json::json!(keys).into(),
                primary_key.as_str().into(),
                playback_position_ticks.into(),
                played_present.into(),
                played.unwrap_or_default().into(),
                increment_play_count.into(),
            ],
        );
        user_data::Model::find_by_statement(statement)
            .one(self.database.as_ref())
            .await?
            .ok_or_else(|| {
                DbErr::RecordNotFound("playback stop upsert returned no row".to_owned()).into()
            })
    }

    /// Marks every alternate video version played for a user.
    ///
    /// `PostgreSQL` resolves the version group from either the primary or an
    /// alternate item, chooses each target version's preferred user-data key,
    /// and upserts all affected rows in one statement. Play count, last played
    /// date, ratings, favorites, and stream selections are preserved.
    ///
    /// # Errors
    ///
    /// Returns a database error when the batch update fails.
    pub async fn mark_alternate_versions_played(
        &self,
        source_item_id: Uuid,
        user_id: Uuid,
        reset_position: bool,
    ) -> Result<Vec<user_data::Model>, UserDataError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            WITH requested AS MATERIALIZED (
                SELECT COALESCE(primary_version_id, id) AS group_id
                FROM jellyfin.base_items
                WHERE id = $1
            ), target_items AS MATERIALIZED (
                SELECT item.id AS item_id, item.presentation_unique_key
                FROM jellyfin.base_items AS item
                INNER JOIN requested
                    ON item.id = requested.group_id
                    OR item.primary_version_id = requested.group_id
                WHERE item.id <> $1
                    AND item.item_type IN (
                        'Video', 'Movie', 'Episode', 'MusicVideo', 'Trailer'
                    )
            ), primary_keys AS (
                SELECT item_id,
                    CASE
                        WHEN btrim(COALESCE(presentation_unique_key, '')) <> ''
                            AND presentation_unique_key <> replace(item_id::text, '-', '')
                            AND presentation_unique_key <> item_id::text
                            THEN presentation_unique_key
                        ELSE replace(item_id::text, '-', '')
                    END AS custom_data_key
                FROM target_items
            ), preferred_keys AS (
                SELECT item_id, custom_data_key, 1 AS priority
                FROM primary_keys
                UNION ALL
                SELECT target.item_id, replace(target.item_id::text, '-', ''), 2 AS priority
                FROM target_items AS target
                INNER JOIN primary_keys AS primary_key USING (item_id)
                WHERE primary_key.custom_data_key <> replace(target.item_id::text, '-', '')
            ), existing_keys AS (
                SELECT DISTINCT ON (data.item_id)
                    data.item_id, data.custom_data_key
                FROM jellyfin.user_data AS data
                INNER JOIN target_items AS target
                    ON target.item_id = data.item_id
                LEFT JOIN preferred_keys AS preferred
                    ON preferred.item_id = data.item_id
                    AND preferred.custom_data_key = data.custom_data_key
                WHERE data.user_id = $2
                ORDER BY data.item_id,
                    preferred.priority NULLS LAST,
                    data.custom_data_key
            ), target_keys AS (
                SELECT target.item_id,
                    COALESCE(existing.custom_data_key, primary_key.custom_data_key)
                        AS custom_data_key
                FROM target_items AS target
                INNER JOIN primary_keys AS primary_key USING (item_id)
                LEFT JOIN existing_keys AS existing USING (item_id)
            )
            INSERT INTO jellyfin.user_data (
                item_id, user_id, custom_data_key,
                playback_position_ticks, played
            )
            SELECT item_id, $2, custom_data_key, 0, true
            FROM target_keys
            ON CONFLICT (item_id, user_id, custom_data_key) DO UPDATE
            SET playback_position_ticks = CASE
                    WHEN $3::boolean THEN 0
                    ELSE jellyfin.user_data.playback_position_ticks
                END,
                played = true
            RETURNING item_id, user_id, custom_data_key, rating,
                playback_position_ticks, play_count, is_favorite,
                last_played_date, played, audio_stream_index,
                subtitle_stream_index, likes, retention_date
            ",
            [source_item_id.into(), user_id.into(), reset_position.into()],
        );
        Ok(user_data::Model::find_by_statement(statement)
            .all(self.database.as_ref())
            .await?)
    }

    /// Clears watched and resume state from existing alternate video versions.
    ///
    /// Unlike marking versions played, Jellyfin does not create missing
    /// alternate user-data rows when propagating an unplayed state. The query
    /// therefore resolves at most one preferred existing row per alternate
    /// version and updates only those rows.
    ///
    /// # Errors
    ///
    /// Returns a database error when the batch update fails.
    pub async fn mark_alternate_versions_unplayed(
        &self,
        source_item_id: Uuid,
        user_id: Uuid,
    ) -> Result<Vec<user_data::Model>, UserDataError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            WITH requested AS MATERIALIZED (
                SELECT COALESCE(primary_version_id, id) AS group_id
                FROM jellyfin.base_items
                WHERE id = $1
            ), target_items AS MATERIALIZED (
                SELECT item.id AS item_id, item.presentation_unique_key
                FROM jellyfin.base_items AS item
                INNER JOIN requested
                    ON item.id = requested.group_id
                    OR item.primary_version_id = requested.group_id
                WHERE item.id <> $1
                    AND item.item_type IN (
                        'Video', 'Movie', 'Episode', 'MusicVideo', 'Trailer'
                    )
            ), primary_keys AS (
                SELECT item_id,
                    CASE
                        WHEN btrim(COALESCE(presentation_unique_key, '')) <> ''
                            AND presentation_unique_key <> replace(item_id::text, '-', '')
                            AND presentation_unique_key <> item_id::text
                            THEN presentation_unique_key
                        ELSE replace(item_id::text, '-', '')
                    END AS custom_data_key
                FROM target_items
            ), preferred_keys AS (
                SELECT item_id, custom_data_key, 1 AS priority
                FROM primary_keys
                UNION ALL
                SELECT target.item_id, replace(target.item_id::text, '-', ''), 2 AS priority
                FROM target_items AS target
                INNER JOIN primary_keys AS primary_key USING (item_id)
                WHERE primary_key.custom_data_key <> replace(target.item_id::text, '-', '')
            ), target_keys AS (
                SELECT DISTINCT ON (data.item_id)
                    data.item_id, data.custom_data_key
                FROM jellyfin.user_data AS data
                INNER JOIN target_items AS target
                    ON target.item_id = data.item_id
                LEFT JOIN preferred_keys AS preferred
                    ON preferred.item_id = data.item_id
                    AND preferred.custom_data_key = data.custom_data_key
                WHERE data.user_id = $2
                ORDER BY data.item_id,
                    preferred.priority NULLS LAST,
                    data.custom_data_key
            )
            UPDATE jellyfin.user_data AS data
            SET playback_position_ticks = 0,
                play_count = 0,
                last_played_date = NULL,
                played = false
            FROM target_keys AS target
            WHERE data.item_id = target.item_id
                AND data.user_id = $2
                AND data.custom_data_key = target.custom_data_key
            RETURNING data.item_id, data.user_id, data.custom_data_key,
                data.rating, data.playback_position_ticks,
                data.play_count, data.is_favorite, data.last_played_date,
                data.played, data.audio_stream_index,
                data.subtitle_stream_index, data.likes, data.retention_date
            ",
            [source_item_id.into(), user_id.into()],
        );
        Ok(user_data::Model::find_by_statement(statement)
            .all(self.database.as_ref())
            .await?)
    }

    /// Atomically marks an item played and resets its resume position.
    ///
    /// Without an explicit date, repeated and concurrent manual toggles keep
    /// the play count at least one. An explicit date represents a new play and
    /// increments the existing count, matching Jellyfin's `MarkPlayed` logic.
    ///
    /// # Errors
    ///
    /// Returns a database error when the upsert fails.
    pub async fn mark_played(
        &self,
        item_id: Uuid,
        user_id: Uuid,
        key: &str,
        date_played: Option<DateTime<Utc>>,
    ) -> Result<user_data::Model, UserDataError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            INSERT INTO jellyfin.user_data (
                item_id, user_id, custom_data_key, playback_position_ticks,
                play_count, last_played_date, played
            )
            VALUES ($1, $2, $3, 0, 1, COALESCE($4::timestamptz, clock_timestamp()), true)
            ON CONFLICT (item_id, user_id, custom_data_key) DO UPDATE
            SET playback_position_ticks = 0,
                play_count = CASE
                    WHEN $4::timestamptz IS NULL
                        THEN GREATEST(jellyfin.user_data.play_count, 1)
                    ELSE jellyfin.user_data.play_count + 1
                END,
                last_played_date = COALESCE(
                    $4::timestamptz,
                    jellyfin.user_data.last_played_date,
                    clock_timestamp()
                ),
                played = true
            RETURNING item_id, user_id, custom_data_key, rating,
                playback_position_ticks, play_count, is_favorite,
                last_played_date, played, audio_stream_index,
                subtitle_stream_index, likes, retention_date
            ",
            [
                item_id.into(),
                user_id.into(),
                key.into(),
                date_played.into(),
            ],
        );
        user_data::Model::find_by_statement(statement)
            .one(self.database.as_ref())
            .await?
            .ok_or_else(|| DbErr::RecordNotFound("mark played returned no row".to_owned()).into())
    }

    /// Atomically clears played state, play count, last-played date, and resume
    /// position while preserving ratings, favorites, and stream selections.
    ///
    /// # Errors
    ///
    /// Returns a database error when the upsert fails.
    pub async fn mark_unplayed(
        &self,
        item_id: Uuid,
        user_id: Uuid,
        key: &str,
    ) -> Result<user_data::Model, UserDataError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            INSERT INTO jellyfin.user_data (
                item_id, user_id, custom_data_key, playback_position_ticks,
                play_count, last_played_date, played
            )
            VALUES ($1, $2, $3, 0, 0, NULL, false)
            ON CONFLICT (item_id, user_id, custom_data_key) DO UPDATE
            SET playback_position_ticks = 0,
                play_count = 0,
                last_played_date = NULL,
                played = false
            RETURNING item_id, user_id, custom_data_key, rating,
                playback_position_ticks, play_count, is_favorite,
                last_played_date, played, audio_stream_index,
                subtitle_stream_index, likes, retention_date
            ",
            [item_id.into(), user_id.into(), key.into()],
        );
        user_data::Model::find_by_statement(statement)
            .one(self.database.as_ref())
            .await?
            .ok_or_else(|| DbErr::RecordNotFound("mark unplayed returned no row".to_owned()).into())
    }

    /// Atomically inserts or replaces one `(item, user, key)` row.
    ///
    /// # Errors
    ///
    /// Returns a validation error for invalid numeric values or a database
    /// error when the upsert fails.
    pub async fn upsert(&self, data: NewUserData) -> Result<user_data::Model, UserDataError> {
        validate_values(data.rating, data.playback_position_ticks, data.play_count)?;
        let active = to_active_model(data);
        Ok(user_data::Entity::insert(active)
            .on_conflict(
                OnConflict::columns([
                    user_data::Column::ItemId,
                    user_data::Column::UserId,
                    user_data::Column::CustomDataKey,
                ])
                .update_columns([
                    user_data::Column::Rating,
                    user_data::Column::PlaybackPositionTicks,
                    user_data::Column::PlayCount,
                    user_data::Column::IsFavorite,
                    user_data::Column::LastPlayedDate,
                    user_data::Column::Played,
                    user_data::Column::AudioStreamIndex,
                    user_data::Column::SubtitleStreamIndex,
                    user_data::Column::Likes,
                    user_data::Column::RetentionDate,
                ])
                .to_owned(),
            )
            .exec_with_returning(self.database.as_ref())
            .await?)
    }

    /// Removes detached user data retained longer than the official window.
    ///
    /// # Errors
    ///
    /// Returns a database error when deletion fails.
    pub async fn delete_detached_before(
        &self,
        item_id: Uuid,
        cutoff: DateTime<Utc>,
    ) -> Result<u64, UserDataError> {
        let result = user_data::Entity::delete_many()
            .filter(user_data::Column::ItemId.eq(item_id))
            .filter(user_data::Column::RetentionDate.lt(cutoff))
            .exec(self.database.as_ref())
            .await?;
        Ok(result.rows_affected)
    }

    /// Loads one exact `(item, user, key)` row.
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn get(
        &self,
        item_id: Uuid,
        user_id: Uuid,
        key: &str,
    ) -> Result<Option<user_data::Model>, UserDataError> {
        Ok(
            user_data::Entity::find_by_id((item_id, user_id, key.to_owned()))
                .one(self.database.as_ref())
                .await?,
        )
    }

    /// Loads all key variants for one user and item.
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn get_for_item(
        &self,
        item_id: Uuid,
        user_id: Uuid,
    ) -> Result<Vec<user_data::Model>, UserDataError> {
        Ok(user_data::Entity::find()
            .filter(user_data::Column::ItemId.eq(item_id))
            .filter(user_data::Column::UserId.eq(user_id))
            .order_by_asc(user_data::Column::CustomDataKey)
            .all(self.database.as_ref())
            .await?)
    }

    /// Resolves current keys in caller-provided priority order, then falls
    /// back to the first retained key row.
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn resolve_by_keys(
        &self,
        item_id: Uuid,
        user_id: Uuid,
        keys: &[String],
    ) -> Result<Option<user_data::Model>, UserDataError> {
        let mut rows = self.get_for_item(item_id, user_id).await?;
        if let Some(index) = keys
            .iter()
            .find_map(|key| rows.iter().position(|row| row.custom_data_key == *key))
        {
            return Ok(rows.swap_remove(index).into());
        }
        Ok(rows.into_iter().next())
    }

    /// Applies selected updates to an existing row.
    ///
    /// # Errors
    ///
    /// Returns a validation error for invalid values or a database error when
    /// lookup or update fails.
    pub async fn patch(
        &self,
        item_id: Uuid,
        user_id: Uuid,
        key: &str,
        patch: UserDataPatch,
    ) -> Result<Option<user_data::Model>, UserDataError> {
        let Some(model) = self.get(item_id, user_id, key).await? else {
            return Ok(None);
        };
        let mut data = from_model(model);
        apply_patch(&mut data, &patch);
        Ok(Some(self.upsert(data).await?))
    }

    /// Runs the real played/favorite/resume/recent user-data filters.
    ///
    /// # Errors
    ///
    /// Returns a database error when loading rows fails.
    pub async fn query(
        &self,
        query: &UserDataQuery,
    ) -> Result<Vec<user_data::Model>, UserDataError> {
        let mut rows =
            user_data::Entity::find().filter(user_data::Column::UserId.eq(query.user_id));
        if !query.item_ids.is_empty() {
            rows = rows.filter(user_data::Column::ItemId.is_in(query.item_ids.iter().copied()));
        }
        if let Some(played) = query.played {
            rows = rows.filter(user_data::Column::Played.eq(played));
        }
        if let Some(is_favorite) = query.is_favorite {
            rows = rows.filter(user_data::Column::IsFavorite.eq(is_favorite));
        }
        if let Some(has_position) = query.has_playback_position {
            rows = if has_position {
                rows.filter(user_data::Column::PlaybackPositionTicks.gt(0))
            } else {
                rows.filter(user_data::Column::PlaybackPositionTicks.eq(0))
            };
        }
        if let Some(min_date) = query.min_last_played_date {
            rows = rows.filter(user_data::Column::LastPlayedDate.gte(min_date));
        }
        if let Some(max_date) = query.max_last_played_date {
            rows = rows.filter(user_data::Column::LastPlayedDate.lte(max_date));
        }
        if query.order_by_last_played_desc {
            rows = rows.order_by(user_data::Column::LastPlayedDate, Order::Desc);
        }
        if let Some(limit) = query.limit {
            rows = rows.limit(limit);
        }
        Ok(rows.all(self.database.as_ref()).await?)
    }
}

fn to_active_model(data: NewUserData) -> user_data::ActiveModel {
    user_data::ActiveModel {
        item_id: Set(data.item_id),
        user_id: Set(data.user_id),
        custom_data_key: Set(data.custom_data_key),
        rating: Set(data.rating),
        playback_position_ticks: Set(data.playback_position_ticks),
        play_count: Set(data.play_count),
        is_favorite: Set(data.is_favorite),
        last_played_date: Set(data.last_played_date),
        played: Set(data.played),
        audio_stream_index: Set(data.audio_stream_index),
        subtitle_stream_index: Set(data.subtitle_stream_index),
        likes: Set(data.likes),
        retention_date: Set(data.retention_date),
    }
}

fn from_model(model: user_data::Model) -> NewUserData {
    NewUserData {
        item_id: model.item_id,
        user_id: model.user_id,
        custom_data_key: model.custom_data_key,
        rating: model.rating,
        playback_position_ticks: model.playback_position_ticks,
        play_count: model.play_count,
        is_favorite: model.is_favorite,
        last_played_date: model.last_played_date,
        played: model.played,
        audio_stream_index: model.audio_stream_index,
        subtitle_stream_index: model.subtitle_stream_index,
        likes: model.likes,
        retention_date: model.retention_date,
    }
}

fn apply_patch(data: &mut NewUserData, patch: &UserDataPatch) {
    if let Some(value) = patch.rating {
        data.rating = value;
    }
    if let Some(value) = patch.playback_position_ticks {
        data.playback_position_ticks = value;
    }
    if let Some(value) = patch.play_count {
        data.play_count = value;
    }
    if let Some(value) = patch.is_favorite {
        data.is_favorite = value;
    }
    if let Some(value) = patch.last_played_date {
        data.last_played_date = value;
    }
    if let Some(value) = patch.played {
        data.played = value;
    }
    if let Some(value) = patch.audio_stream_index {
        data.audio_stream_index = value;
    }
    if let Some(value) = patch.subtitle_stream_index {
        data.subtitle_stream_index = value;
    }
    if let Some(value) = patch.likes {
        data.likes = value;
    }
    if let Some(value) = patch.retention_date {
        data.retention_date = value;
    }
}

fn validate_values(rating: Option<f64>, position: i64, count: i32) -> Result<(), UserDataError> {
    if rating.is_some_and(|rating| !(0.0..=10.0).contains(&rating)) {
        return Err(UserDataError::InvalidRating);
    }
    if position < 0 || count < 0 {
        return Err(UserDataError::NegativePlaybackValue);
    }
    Ok(())
}

fn validate_optional_values(
    rating: Option<f64>,
    position: Option<i64>,
    count: Option<i32>,
) -> Result<(), UserDataError> {
    if rating.is_some_and(|rating| !(0.0..=10.0).contains(&rating)) {
        return Err(UserDataError::InvalidRating);
    }
    if position.is_some_and(|position| position < 0) || count.is_some_and(|count| count < 0) {
        return Err(UserDataError::NegativePlaybackValue);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_rating_and_playback_values() {
        assert!(matches!(
            validate_values(Some(10.1), 0, 0),
            Err(UserDataError::InvalidRating)
        ));
        assert!(matches!(
            validate_values(None, -1, 0),
            Err(UserDataError::NegativePlaybackValue)
        ));
        assert!(matches!(
            validate_optional_values(Some(f64::NAN), None, None),
            Err(UserDataError::InvalidRating)
        ));
        assert!(matches!(
            validate_optional_values(None, None, Some(-1)),
            Err(UserDataError::NegativePlaybackValue)
        ));
    }
}
