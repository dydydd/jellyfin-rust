use chrono::Utc;
use jellyfin_data::entities::user;
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder, Set,
    TryInsertResult, sea_query::OnConflict,
};
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

const DEFAULT_AUTH_PROVIDER: &str =
    "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider";

#[derive(Debug, Error)]
pub enum UserError {
    #[error("invalid username")]
    InvalidUsername,
    #[error("a user with the name '{0}' already exists")]
    DuplicateUsername(String),
    #[error("user not found")]
    NotFound,
    #[error(transparent)]
    Database(#[from] DbErr),
}

#[derive(Clone)]
pub struct UserService {
    database: DatabaseConnection,
}

impl UserService {
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Creates a user, atomically rejecting a case-insensitive duplicate.
    ///
    /// # Errors
    ///
    /// Returns [`UserError::InvalidUsername`] for invalid input,
    /// [`UserError::DuplicateUsername`] when the normalized name already
    /// exists, or [`UserError::Database`] when the insert fails.
    pub async fn create(&self, name: &str) -> Result<user::Model, UserError> {
        validate_username(name)?;
        let normalized = normalize_username(name);

        let now = Utc::now();
        let result = user::Entity::insert(user::ActiveModel {
            id: Set(Uuid::new_v4()),
            username: Set(name.to_owned()),
            normalized_username: Set(normalized),
            password_hash: Set(None),
            must_update_password: Set(false),
            is_administrator: Set(false),
            is_hidden: Set(true),
            is_disabled: Set(false),
            enable_auto_login: Set(false),
            last_login_date: Set(None),
            last_activity_date: Set(None),
            policy: Set(json!({
                "AuthenticationProviderId": DEFAULT_AUTH_PROVIDER,
                "EnableUserPreferenceAccess": true
            })),
            preferences: Set(json!({
                "RememberAudioSelections": true,
                "RememberSubtitleSelections": true,
                "EnableNextEpisodeAutoPlay": true
            })),
            row_version: Set(1),
            created_at: Set(now),
            updated_at: Set(now),
        })
        .on_conflict(
            OnConflict::column(user::Column::NormalizedUsername)
                .do_nothing()
                .to_owned(),
        )
        .do_nothing()
        .exec_with_returning(&self.database)
        .await;

        // SeaORM 1.1 converts a zero-row `DO NOTHING RETURNING` result into
        // `RecordNotFound` before its `TryInsertResult` adapter sees it.
        match result {
            Ok(TryInsertResult::Inserted(inserted)) => Ok(inserted),
            Ok(TryInsertResult::Conflicted) | Err(DbErr::RecordNotFound(_)) => {
                Err(UserError::DuplicateUsername(name.to_owned()))
            }
            Ok(TryInsertResult::Empty) => Err(DbErr::RecordNotInserted.into()),
            Err(error) => Err(error.into()),
        }
    }

    /// Retrieves a user by identifier.
    ///
    /// # Errors
    ///
    /// Returns [`UserError::NotFound`] when no user has `id`, or
    /// [`UserError::Database`] when the query fails.
    pub async fn get(&self, id: Uuid) -> Result<user::Model, UserError> {
        user::Entity::find_by_id(id)
            .one(&self.database)
            .await?
            .ok_or(UserError::NotFound)
    }

    /// Retrieves a user by case-insensitive username.
    ///
    /// # Errors
    ///
    /// Returns [`UserError::InvalidUsername`] for blank input or
    /// [`UserError::Database`] when the query fails.
    pub async fn get_by_name(&self, name: &str) -> Result<Option<user::Model>, UserError> {
        if name.trim().is_empty() {
            return Err(UserError::InvalidUsername);
        }
        Ok(user::Entity::find()
            .filter(user::Column::NormalizedUsername.eq(normalize_username(name)))
            .one(&self.database)
            .await?)
    }

    /// Lists all users in normalized username order.
    ///
    /// # Errors
    ///
    /// Returns [`UserError::Database`] when the query fails.
    pub async fn list(&self) -> Result<Vec<user::Model>, UserError> {
        Ok(user::Entity::find()
            .order_by_asc(user::Column::NormalizedUsername)
            .all(&self.database)
            .await?)
    }

    /// Lists visible, enabled users in normalized username order.
    ///
    /// # Errors
    ///
    /// Returns [`UserError::Database`] when the query fails.
    pub async fn list_public(&self) -> Result<Vec<user::Model>, UserError> {
        Ok(user::Entity::find()
            .filter(user::Column::IsHidden.eq(false))
            .filter(user::Column::IsDisabled.eq(false))
            .order_by_asc(user::Column::NormalizedUsername)
            .all(&self.database)
            .await?)
    }
}

fn normalize_username(name: &str) -> String {
    name.to_uppercase()
}

/// Validates a username against Jellyfin's supported character rules.
///
/// # Errors
///
/// Returns [`UserError::InvalidUsername`] when `name` is empty, reserved,
/// surrounded by whitespace, or contains an unsupported character.
pub fn validate_username(name: &str) -> Result<(), UserError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.trim() != name
        || !name.chars().all(is_allowed_username_character)
    {
        return Err(UserError::InvalidUsername);
    }
    Ok(())
}

fn is_allowed_username_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '-' | '\'' | '.' | '@' | '+' | ' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_valid_usernames_are_accepted() {
        for value in [
            "this_is_valid",
            "this is also valid",
            "0@_-' .",
            "Aa0@_-' .+",
            "thisisa+testemail@test.foo",
            "münchen",
            "Ñoño",
        ] {
            assert!(validate_username(value).is_ok(), "{value}");
        }
    }

    #[test]
    fn official_invalid_usernames_are_rejected() {
        for value in [
            "",
            " ",
            ".",
            "..",
            " leading",
            "trailing ",
            "contains & invalid",
            "‼️",
        ] {
            assert!(validate_username(value).is_err(), "{value}");
        }
    }

    #[test]
    fn normalization_matches_official_examples() {
        assert_eq!(normalize_username("münchen"), "MÜNCHEN");
        assert_eq!(normalize_username("Ñoño"), "ÑOÑO");
        assert_eq!(normalize_username("Çelebi"), "ÇELEBI");
    }
}
