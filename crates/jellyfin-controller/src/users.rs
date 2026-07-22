use chrono::Utc;
use jellyfin_data::entities::user;
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, DbBackend, DbErr, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, Set, SqlErr, Statement, TransactionTrait,
    TryInsertResult,
    sea_query::{Expr, OnConflict},
};
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

const DEFAULT_AUTH_PROVIDER: &str =
    "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider";
const USER_MUTATION_LOCK_KEY: i64 = 0x4a45_4c4c_5955_5345;

#[derive(Debug, Error)]
pub enum UserError {
    #[error("invalid username")]
    InvalidUsername,
    #[error("a user with the name '{0}' already exists")]
    DuplicateUsername(String),
    #[error("user not found")]
    NotFound,
    #[error("the user already has a configured password")]
    PasswordAlreadyConfigured,
    #[error("there must be at least one user")]
    LastUser,
    #[error("there must be at least one administrator")]
    LastAdministrator,
    #[error("administrator passwords must not be empty")]
    AdministratorPasswordRequired,
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
        self.create_with_role(name, false).await
    }

    /// Creates the initial administrator account.
    ///
    /// # Errors
    ///
    /// Returns the same validation, duplicate-name, and persistence errors as
    /// [`Self::create`].
    pub async fn create_initial_administrator(&self, name: &str) -> Result<user::Model, UserError> {
        self.create_with_role(name, true).await
    }

    async fn create_with_role(
        &self,
        name: &str,
        is_administrator: bool,
    ) -> Result<user::Model, UserError> {
        validate_username(name)?;
        let normalized = normalize_username(name);

        let now = Utc::now();
        let result = user::Entity::insert(user::ActiveModel {
            id: Set(Uuid::new_v4()),
            username: Set(name.to_owned()),
            normalized_username: Set(normalized),
            password_hash: Set(None),
            must_update_password: Set(false),
            is_administrator: Set(is_administrator),
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

    /// Retrieves the first user in stable creation order.
    ///
    /// # Errors
    ///
    /// Returns [`UserError::Database`] when the query fails.
    pub async fn first(&self) -> Result<Option<user::Model>, UserError> {
        Ok(user::Entity::find()
            .order_by_asc(user::Column::CreatedAt)
            .order_by_asc(user::Column::Id)
            .one(&self.database)
            .await?)
    }

    /// Atomically configures the initial user's name and first password hash.
    ///
    /// # Errors
    ///
    /// Returns a validation or duplicate-name error for an invalid name,
    /// [`UserError::PasswordAlreadyConfigured`] if a password won the race to
    /// configure this user, [`UserError::NotFound`] when the user disappeared,
    /// or [`UserError::Database`] for other persistence failures.
    pub async fn configure_startup_user(
        &self,
        id: Uuid,
        name: &str,
        password_hash: &str,
    ) -> Result<user::Model, UserError> {
        validate_username(name)?;
        let update = user::Entity::update_many()
            .col_expr(user::Column::Username, Expr::value(name))
            .col_expr(
                user::Column::NormalizedUsername,
                Expr::value(normalize_username(name)),
            )
            .col_expr(user::Column::PasswordHash, Expr::value(password_hash))
            .filter(user::Column::Id.eq(id))
            .filter(
                Condition::any()
                    .add(user::Column::PasswordHash.is_null())
                    .add(user::Column::PasswordHash.eq("")),
            )
            .exec(&self.database)
            .await;

        match update {
            Ok(result) if result.rows_affected == 1 => self.get(id).await,
            Ok(_) => {
                if user::Entity::find_by_id(id)
                    .one(&self.database)
                    .await?
                    .is_some()
                {
                    Err(UserError::PasswordAlreadyConfigured)
                } else {
                    Err(UserError::NotFound)
                }
            }
            Err(error) if matches!(error.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) => {
                Err(UserError::DuplicateUsername(name.to_owned()))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Persists a successful local authentication, including a migrated hash.
    ///
    /// # Errors
    ///
    /// Returns [`UserError::NotFound`] when the user disappeared, or
    /// [`UserError::Database`] when the update fails.
    pub async fn record_successful_authentication(
        &self,
        authenticated_user: &user::Model,
    ) -> Result<user::Model, UserError> {
        let now = Utc::now();
        let result = user::Entity::update_many()
            .col_expr(
                user::Column::PasswordHash,
                Expr::value(authenticated_user.password_hash.clone()),
            )
            .col_expr(user::Column::LastLoginDate, Expr::value(now))
            .col_expr(user::Column::LastActivityDate, Expr::value(now))
            .filter(user::Column::Id.eq(authenticated_user.id))
            .exec(&self.database)
            .await?;
        if result.rows_affected == 0 {
            return Err(UserError::NotFound);
        }
        self.get(authenticated_user.id).await
    }

    /// Renames a user while preserving case-insensitive uniqueness.
    ///
    /// # Errors
    ///
    /// Returns a validation or duplicate-name error, [`UserError::NotFound`],
    /// or [`UserError::Database`].
    pub async fn rename(&self, id: Uuid, name: &str) -> Result<user::Model, UserError> {
        validate_username(name)?;
        let update = user::Entity::update_many()
            .col_expr(user::Column::Username, Expr::value(name))
            .col_expr(
                user::Column::NormalizedUsername,
                Expr::value(normalize_username(name)),
            )
            .filter(user::Column::Id.eq(id))
            .exec(&self.database)
            .await;
        match update {
            Ok(result) if result.rows_affected == 1 => self.get(id).await,
            Ok(_) => Err(UserError::NotFound),
            Err(error) if matches!(error.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) => {
                Err(UserError::DuplicateUsername(name.to_owned()))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Changes or clears a user's password hash.
    ///
    /// # Errors
    ///
    /// Returns [`UserError::AdministratorPasswordRequired`] when clearing an
    /// administrator password, [`UserError::NotFound`] when no row matches, or
    /// [`UserError::Database`] when persistence fails.
    pub async fn set_password_hash(
        &self,
        id: Uuid,
        password_hash: Option<String>,
    ) -> Result<user::Model, UserError> {
        let existing = self.get(id).await?;
        if existing.is_administrator && password_hash.is_none() {
            return Err(UserError::AdministratorPasswordRequired);
        }
        let result = user::Entity::update_many()
            .col_expr(user::Column::PasswordHash, Expr::value(password_hash))
            .filter(user::Column::Id.eq(id))
            .exec(&self.database)
            .await?;
        if result.rows_affected == 0 {
            return Err(UserError::NotFound);
        }
        self.get(id).await
    }

    /// Deletes a user while preserving Jellyfin's last-user and last-admin
    /// invariants under concurrent `PostgreSQL` requests.
    ///
    /// # Errors
    ///
    /// Returns [`UserError::NotFound`], [`UserError::LastUser`],
    /// [`UserError::LastAdministrator`], or [`UserError::Database`].
    pub async fn delete(&self, id: Uuid) -> Result<(), UserError> {
        let transaction = self.database.begin().await?;
        transaction
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT pg_advisory_xact_lock($1)",
                [USER_MUTATION_LOCK_KEY.into()],
            ))
            .await?;
        let target = user::Entity::find_by_id(id)
            .one(&transaction)
            .await?
            .ok_or(UserError::NotFound)?;
        if user::Entity::find().count(&transaction).await? <= 1 {
            return Err(UserError::LastUser);
        }
        if target.is_administrator
            && user::Entity::find()
                .filter(user::Column::IsAdministrator.eq(true))
                .count(&transaction)
                .await?
                <= 1
        {
            return Err(UserError::LastAdministrator);
        }
        user::Entity::delete_by_id(id).exec(&transaction).await?;
        transaction.commit().await?;
        Ok(())
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
