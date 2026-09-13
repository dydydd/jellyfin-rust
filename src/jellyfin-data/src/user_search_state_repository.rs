use chrono::Utc;
use sea_orm::{ColumnTrait, DbErr, EntityTrait, QueryFilter, Set, sea_query::OnConflict};
use uuid::Uuid;

use crate::entities::user_search_state;

/// PostgreSQL-backed Emby item-search reporting state.
///
/// The generated Emby contract reports only a nullable `WasSearched` flag;
/// it does not carry a search term or item identifier. Consequently this is
/// one current state row per user, rather than an invented search-term log.
#[derive(Clone)]
pub struct UserSearchStateRepository {
    database: crate::SharedDatabase,
}

impl UserSearchStateRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Loads the current search-reporting state for one user.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn get(&self, user_id: Uuid) -> Result<Option<user_search_state::Model>, DbErr> {
        user_search_state::Entity::find_by_id(user_id)
            .one(self.database.as_ref())
            .await
    }

    /// Atomically inserts or updates one user's search-reporting state.
    ///
    /// # Errors
    ///
    /// Returns a database error, including a foreign-key failure for an
    /// unknown user.
    pub async fn set(
        &self,
        user_id: Uuid,
        was_searched: bool,
    ) -> Result<user_search_state::Model, DbErr> {
        let now = Utc::now();
        user_search_state::Entity::insert(user_search_state::ActiveModel {
            user_id: Set(user_id),
            was_searched: Set(was_searched),
            updated_at: Set(now),
        })
        .on_conflict(
            OnConflict::column(user_search_state::Column::UserId)
                .update_columns([
                    user_search_state::Column::WasSearched,
                    user_search_state::Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec_with_returning(self.database.as_ref())
        .await
    }

    /// Deletes one user's search-reporting state. Repeated clears are
    /// intentionally idempotent.
    ///
    /// # Errors
    ///
    /// Returns a database error when the deletion fails.
    pub async fn clear(&self, user_id: Uuid) -> Result<(), DbErr> {
        user_search_state::Entity::delete_many()
            .filter(user_search_state::Column::UserId.eq(user_id))
            .exec(self.database.as_ref())
            .await?;
        Ok(())
    }
}
