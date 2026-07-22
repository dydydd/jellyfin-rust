use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, Order, QueryFilter, QueryOrder,
    QuerySelect, Set, sea_query::OnConflict,
};
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

#[derive(Debug, Error)]
pub enum UserDataError {
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
    database: DatabaseConnection,
}

impl UserDataRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
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
            .exec_with_returning(&self.database)
            .await?)
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
                .one(&self.database)
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
            .all(&self.database)
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
        let rows = self.get_for_item(item_id, user_id).await?;
        for key in keys {
            if let Some(row) = rows.iter().find(|row| row.custom_data_key == *key) {
                return Ok(Some(row.clone()));
            }
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
        Ok(rows.all(&self.database).await?)
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
    }
}
