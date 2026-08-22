use std::fmt::Write as _;

use chrono::{Duration, Utc};
use jellyfin_data::{
    NewUserProfileImage, PlaylistUserPermission, UserProfileImageRepository,
    UserProfileImageStoreError,
    entities::{base_item, linked_child, password_reset, playlist, user, user_profile_image},
};
use jellyfin_model::{
    ForgotPasswordAction, ForgotPasswordResult, NameIdPair, UserConfiguration, UserPolicy,
};
use md5::{Digest, Md5};
use sea_orm::{
    ActiveValue::NotSet,
    ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, DbBackend, DbErr, EntityTrait,
    FromQueryResult, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set, SqlErr, Statement,
    TransactionTrait, TryInsertResult,
    sea_query::Value,
    sea_query::{Expr, OnConflict},
};
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

const USER_MUTATION_LOCK_KEY: i64 = 0x4a45_4c4c_5955_5345;
const DEFAULT_AUTHENTICATION_PROVIDER_NAME: &str = "Default";
const DEFAULT_PASSWORD_RESET_PROVIDER_NAME: &str = "Default Password Reset Provider";

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
    #[error("administrator accounts cannot be disabled")]
    AdministratorCannotBeDisabled,
    #[error("there must be at least one enabled user")]
    LastEnabledUser,
    #[error("invalid user policy")]
    InvalidPolicy,
    #[error("administrator passwords must not be empty")]
    AdministratorPasswordRequired,
    #[error("password reset pin not found")]
    PasswordResetPinNotFound,
    #[error("failed to serialize user configuration")]
    ConfigurationSerialization(#[source] serde_json::Error),
    #[error("failed to serialize user policy")]
    PolicySerialization(#[source] serde_json::Error),
    #[error("stored playlist shares are invalid")]
    CorruptPlaylistShares,
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

    /// Lists enabled authentication providers in Jellyfin's official order.
    #[allow(
        clippy::unused_self,
        reason = "provider registration is a UserService concern and will use instance state once plugin-backed providers are wired in"
    )]
    pub fn authentication_providers(&self) -> Vec<NameIdPair> {
        vec![NameIdPair {
            name: DEFAULT_AUTHENTICATION_PROVIDER_NAME.to_owned(),
            id: UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned(),
        }]
    }

    /// Lists enabled password reset providers in Jellyfin's official order.
    #[allow(
        clippy::unused_self,
        reason = "provider registration is a UserService concern and will use instance state once plugin-backed providers are wired in"
    )]
    pub fn password_reset_providers(&self) -> Vec<NameIdPair> {
        vec![NameIdPair {
            name: DEFAULT_PASSWORD_RESET_PROVIDER_NAME.to_owned(),
            id: UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned(),
        }]
    }

    /// Starts Jellyfin's default PIN-based password reset flow.
    ///
    /// The official provider persists a `passwordreset*.json` file. This Rust
    /// port keeps the same public API shape while storing the active PIN in
    /// `PostgreSQL`, keyed by the same Jellyfin MD5 username hash.
    ///
    /// # Errors
    ///
    /// Returns a database error when the reset row cannot be persisted.
    pub async fn start_forgot_password_process(
        &self,
        entered_username: &str,
        is_in_network: bool,
    ) -> Result<ForgotPasswordResult, UserError> {
        let now = Utc::now();
        let expires_at = now + Duration::minutes(30);
        let username_hash = jellyfin_md5(&entered_username.to_uppercase());
        let pin_file = format!("passwordreset{username_hash}.json");

        let user = if entered_username.trim().is_empty() {
            None
        } else {
            user::Entity::find()
                .filter(user::Column::NormalizedUsername.eq(normalize_username(entered_username)))
                .one(&self.database)
                .await?
        };

        if let Some(user) = user.filter(|_| is_in_network) {
            let pin = generate_pin();
            let pin_compact = compact_pin(&pin);
            password_reset::Entity::insert(password_reset::ActiveModel {
                id: NotSet,
                username_hash: Set(username_hash),
                user_id: Set(user.id),
                user_name: Set(user.username),
                pin: Set(pin),
                pin_compact: Set(pin_compact),
                pin_file: Set(pin_file.clone()),
                created_at: Set(now),
                expires_at: Set(expires_at),
            })
            .on_conflict(
                OnConflict::column(password_reset::Column::UsernameHash)
                    .update_columns([
                        password_reset::Column::UserId,
                        password_reset::Column::UserName,
                        password_reset::Column::Pin,
                        password_reset::Column::PinCompact,
                        password_reset::Column::PinFile,
                        password_reset::Column::CreatedAt,
                        password_reset::Column::ExpiresAt,
                    ])
                    .to_owned(),
            )
            .exec(&self.database)
            .await?;
        }

        Ok(ForgotPasswordResult {
            action: ForgotPasswordAction::PinCode,
            pin_file: Some(pin_file),
            pin_expiration_date: Some(expires_at),
        })
    }

    /// Redeems a password reset PIN and updates every matching user.
    ///
    /// Expired rows are purged in the same `PostgreSQL` transaction. Matching
    /// rows are locked before password update and deletion, so concurrent
    /// redemption attempts cannot consume the same PIN twice.
    ///
    /// # Errors
    ///
    /// Returns [`UserError::PasswordResetPinNotFound`] for unknown or expired
    /// PINs, or a database error when persistence fails.
    pub async fn redeem_password_reset_pin(
        &self,
        pin: &str,
        password_hash: Option<String>,
    ) -> Result<Vec<String>, UserError> {
        let pin_compact = compact_pin(pin);
        let now = Utc::now();
        let transaction = self.database.begin().await?;

        password_reset::Entity::delete_many()
            .filter(password_reset::Column::ExpiresAt.lt(now))
            .exec(&transaction)
            .await?;

        let resets = password_reset::Entity::find()
            .filter(password_reset::Column::PinCompact.eq(pin_compact))
            .filter(password_reset::Column::ExpiresAt.gte(now))
            .lock_exclusive()
            .all(&transaction)
            .await?;
        if resets.is_empty() {
            transaction.commit().await?;
            return Err(UserError::PasswordResetPinNotFound);
        }

        let user_ids = resets.iter().map(|reset| reset.user_id).collect::<Vec<_>>();
        let reset_ids = resets.iter().map(|reset| reset.id).collect::<Vec<_>>();
        let users_reset = resets
            .iter()
            .map(|reset| reset.user_name.clone())
            .collect::<Vec<_>>();
        let updated = user::Entity::update_many()
            .col_expr(user::Column::PasswordHash, Expr::value(password_hash))
            .filter(user::Column::Id.is_in(user_ids))
            .exec(&transaction)
            .await?;
        if updated.rows_affected == 0 {
            transaction.commit().await?;
            return Err(UserError::PasswordResetPinNotFound);
        }

        password_reset::Entity::delete_many()
            .filter(password_reset::Column::Id.is_in(reset_ids))
            .exec(&transaction)
            .await?;
        transaction.commit().await?;
        Ok(users_reset)
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
        let policy = UserPolicy {
            is_administrator,
            authentication_provider_id: Some(
                UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned(),
            ),
            password_reset_provider_id: Some(
                UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned(),
            ),
            ..UserPolicy::default()
        };
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
            authentication_provider_id: Set(
                UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()
            ),
            password_reset_provider_id: Set(
                UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()
            ),
            policy: Set(serde_json::to_value(policy).map_err(UserError::PolicySerialization)?),
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

    /// Loads the persisted profile image for a user.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn profile_image(
        &self,
        user_id: Uuid,
    ) -> Result<Option<user_profile_image::Model>, UserProfileImageStoreError> {
        UserProfileImageRepository::new(self.database.clone())
            .get(user_id)
            .await
    }

    /// Atomically inserts or replaces a user's persisted profile image.
    ///
    /// # Errors
    ///
    /// Returns a profile-image validation or database error.
    pub async fn set_profile_image(
        &self,
        image: NewUserProfileImage,
    ) -> Result<user_profile_image::Model, UserProfileImageStoreError> {
        UserProfileImageRepository::new(self.database.clone())
            .upsert(image)
            .await
    }

    /// Clears a user's persisted profile image by user ID.
    ///
    /// This deliberately does not accept an image key, so a detached image
    /// carrying a temporary key cannot prevent removal of the persisted row.
    /// A missing image is an idempotent no-op.
    ///
    /// # Errors
    ///
    /// Returns a database error when deletion fails.
    pub async fn clear_profile_image(
        &self,
        user_id: Uuid,
    ) -> Result<Option<user_profile_image::Model>, UserProfileImageStoreError> {
        UserProfileImageRepository::new(self.database.clone())
            .clear(user_id)
            .await
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
        let mut policy: UserPolicy =
            serde_json::from_value(authenticated_user.policy.clone())
                .map_err(UserError::PolicySerialization)?;
        policy.invalid_login_attempt_count = 0;
        let policy =
            serde_json::to_value(policy).map_err(UserError::PolicySerialization)?;
        let result = user::Entity::update_many()
            .col_expr(
                user::Column::PasswordHash,
                Expr::value(authenticated_user.password_hash.clone()),
            )
            .col_expr(user::Column::LastLoginDate, Expr::value(now))
            .col_expr(user::Column::LastActivityDate, Expr::value(now))
            .col_expr(user::Column::Policy, Expr::value(policy))
            .filter(user::Column::Id.eq(authenticated_user.id))
            .exec(&self.database)
            .await?;
        if result.rows_affected == 0 {
            return Err(UserError::NotFound);
        }
        self.get(authenticated_user.id).await
    }

    /// Persists one failed local authentication attempt and applies Jellyfin's
    /// lockout policy when the configured threshold is reached.
    ///
    /// Administrator accounts are not disabled by the lockout path because
    /// this port preserves the last-administrator invariant.
    ///
    /// # Errors
    ///
    /// Returns [`UserError::NotFound`] when the user disappeared, or a
    /// serialization or database error when the update fails.
    pub async fn record_failed_authentication(
        &self,
        id: Uuid,
    ) -> Result<user::Model, UserError> {
        let transaction = self.database.begin().await?;
        let target = user::Entity::find_by_id(id)
            .lock_exclusive()
            .one(&transaction)
            .await?
            .ok_or(UserError::NotFound)?;
        let mut policy: UserPolicy =
            serde_json::from_value(target.policy.clone()).map_err(UserError::PolicySerialization)?;
        let attempts = policy.invalid_login_attempt_count.saturating_add(1);
        policy.invalid_login_attempt_count = attempts;
        let lockout = policy.login_attempts_before_lockout > 0
            && attempts >= policy.login_attempts_before_lockout;
        let is_disabled = if lockout && !target.is_administrator {
            policy.is_disabled = true;
            true
        } else {
            target.is_disabled
        };
        let policy = serde_json::to_value(policy).map_err(UserError::PolicySerialization)?;
        user::Entity::update_many()
            .col_expr(user::Column::Policy, Expr::value(policy))
            .col_expr(user::Column::IsDisabled, Expr::value(is_disabled))
            .filter(user::Column::Id.eq(id))
            .exec(&transaction)
            .await?;
        transaction.commit().await?;
        self.get(id).await
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
        remove_user_from_playlists(&transaction, id).await?;
        user::Entity::delete_by_id(id).exec(&transaction).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Atomically replaces a user's policy while preserving the global
    /// administrator and enabled-user invariants.
    ///
    /// Returns the updated user and whether the update disabled an account
    /// that was previously enabled.
    ///
    /// # Errors
    ///
    /// Returns [`UserError::InvalidPolicy`] for missing, blank, or oversized
    /// provider identifiers; an invariant error when the update would disable
    /// an administrator or remove the last administrator or enabled user;
    /// [`UserError::NotFound`] when the user does not exist; or a persistence
    /// error.
    pub async fn update_policy(
        &self,
        id: Uuid,
        policy: &UserPolicy,
    ) -> Result<(user::Model, bool), UserError> {
        let authentication_provider_id =
            validate_policy_provider_id(policy.authentication_provider_id.as_deref())?;
        let password_reset_provider_id =
            validate_policy_provider_id(policy.password_reset_provider_id.as_deref())?;
        let serialized = serde_json::to_value(policy).map_err(UserError::PolicySerialization)?;

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

        if target.is_administrator && policy.is_disabled {
            return Err(UserError::AdministratorCannotBeDisabled);
        }
        if target.is_administrator
            && !policy.is_administrator
            && user::Entity::find()
                .filter(user::Column::IsAdministrator.eq(true))
                .count(&transaction)
                .await?
                <= 1
        {
            return Err(UserError::LastAdministrator);
        }

        let became_disabled = !target.is_disabled && policy.is_disabled;
        if became_disabled
            && user::Entity::find()
                .filter(user::Column::IsDisabled.eq(false))
                .count(&transaction)
                .await?
                <= 1
        {
            return Err(UserError::LastEnabledUser);
        }

        let result = user::Entity::update_many()
            .col_expr(user::Column::Policy, Expr::value(serialized))
            .col_expr(
                user::Column::AuthenticationProviderId,
                Expr::value(authentication_provider_id),
            )
            .col_expr(
                user::Column::PasswordResetProviderId,
                Expr::value(password_reset_provider_id),
            )
            .col_expr(
                user::Column::IsAdministrator,
                Expr::value(policy.is_administrator),
            )
            .col_expr(user::Column::IsHidden, Expr::value(policy.is_hidden))
            .col_expr(user::Column::IsDisabled, Expr::value(policy.is_disabled))
            .filter(user::Column::Id.eq(id))
            .exec(&transaction)
            .await?;
        if result.rows_affected == 0 {
            return Err(UserError::NotFound);
        }
        let updated = user::Entity::find_by_id(id)
            .one(&transaction)
            .await?
            .ok_or(UserError::NotFound)?;
        transaction.commit().await?;
        Ok((updated, became_disabled))
    }

    /// Replaces a user's client configuration preferences.
    ///
    /// The official server stores these settings in per-user configuration
    /// files. This Rust port keeps `PostgreSQL` as the source of truth by
    /// storing the same wire contract in the `users.preferences` `jsonb`
    /// column.
    ///
    /// # Errors
    ///
    /// Returns [`UserError::NotFound`] when the user does not exist, a
    /// serialization error when the DTO cannot be represented as JSON, or a
    /// persistence error.
    pub async fn update_configuration(
        &self,
        id: Uuid,
        configuration: &UserConfiguration,
    ) -> Result<user::Model, UserError> {
        let serialized =
            serde_json::to_value(configuration).map_err(UserError::ConfigurationSerialization)?;
        let result = user::Entity::update_many()
            .col_expr(user::Column::Preferences, Expr::value(serialized))
            .col_expr(user::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(user::Column::Id.eq(id))
            .exec(&self.database)
            .await?;
        if result.rows_affected == 0 {
            return Err(UserError::NotFound);
        }
        self.get(id).await
    }

    /// Lists all users in normalized username order.
    ///
    /// # Errors
    ///
    /// Returns [`UserError::Database`] when the query fails.
    pub async fn list(&self) -> Result<Vec<user::Model>, UserError> {
        self.list_filtered(None, None).await
    }

    /// Lists users matching optional hidden/disabled filters in normalized
    /// username order.
    ///
    /// # Errors
    ///
    /// Returns [`UserError::Database`] when the query fails.
    pub async fn list_filtered(
        &self,
        is_hidden: Option<bool>,
        is_disabled: Option<bool>,
    ) -> Result<Vec<user::Model>, UserError> {
        let mut query = user::Entity::find();
        if let Some(is_hidden) = is_hidden {
            query = query.filter(user::Column::IsHidden.eq(is_hidden));
        }
        if let Some(is_disabled) = is_disabled {
            query = query.filter(user::Column::IsDisabled.eq(is_disabled));
        }
        Ok(query
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

async fn remove_user_from_playlists<C>(database: &C, user_id: Uuid) -> Result<(), UserError>
where
    C: ConnectionTrait,
{
    let rows = playlist::Model::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT playlist_id, owner_user_id, open_access, media_type, shares \
         FROM jellyfin.playlists \
         WHERE owner_user_id = $1::uuid OR shares @> $2::jsonb \
         FOR UPDATE",
        [
            user_id.into(),
            serde_json::json!([{ "UserId": user_id }]).into(),
        ],
    ))
    .all(database)
    .await?;
    for row in rows {
        let mut shares = serde_json::from_value::<Vec<PlaylistUserPermission>>(row.shares)
            .map_err(|_| UserError::CorruptPlaylistShares)?;
        shares.retain(|share| share.user_id != user_id);
        let mut owner_user_id = row.owner_user_id;
        if owner_user_id == Some(user_id) {
            shares.sort_by_key(|share| !share.can_edit);
            if let Some(new_owner) = shares.first().copied() {
                owner_user_id = Some(new_owner.user_id);
                shares.remove(0);
            } else if !row.open_access {
                linked_child::Entity::delete_many()
                    .filter(linked_child::Column::ParentId.eq(row.playlist_id))
                    .exec(database)
                    .await?;
                base_item::Entity::delete_by_id(row.playlist_id)
                    .exec(database)
                    .await?;
                continue;
            } else {
                owner_user_id = None;
            }
        }
        database
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE jellyfin.playlists \
                 SET owner_user_id = $1::uuid, shares = $2::jsonb \
                 WHERE playlist_id = $3::uuid",
                [
                    Value::Uuid(owner_user_id.map(Box::new)),
                    serde_json::to_value(shares)
                        .map_err(|_| UserError::CorruptPlaylistShares)?
                        .into(),
                    row.playlist_id.into(),
                ],
            ))
            .await?;
    }
    Ok(())
}

fn normalize_username(name: &str) -> String {
    name.to_uppercase()
}

fn compact_pin(pin: &str) -> String {
    pin.replace('-', "")
}

fn generate_pin() -> String {
    let bytes = *Uuid::new_v4().as_bytes();
    format!(
        "{:02X}-{:02X}-{:02X}-{:02X}",
        bytes[0], bytes[1], bytes[2], bytes[3]
    )
}

fn jellyfin_md5(value: &str) -> String {
    let mut hasher = Md5::new();
    for unit in value.encode_utf16() {
        hasher.update(unit.to_le_bytes());
    }
    let digest = hasher.finalize();
    let bytes = digest.as_slice();
    let mut result = format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[3], bytes[2], bytes[1], bytes[0], bytes[5], bytes[4], bytes[7], bytes[6]
    );
    for byte in &bytes[8..] {
        write!(result, "{byte:02x}").expect("writing to a String cannot fail");
    }
    result
}

fn validate_policy_provider_id(value: Option<&str>) -> Result<String, UserError> {
    let value = value.ok_or(UserError::InvalidPolicy)?;
    if value.trim().is_empty() || value.chars().count() > 255 {
        return Err(UserError::InvalidPolicy);
    }
    Ok(value.to_owned())
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

    #[test]
    fn password_reset_username_hash_matches_official_guid_md5_format() {
        assert_eq!(jellyfin_md5("USER"), "2d7db5040a259018420769625d251673");
        assert_eq!(
            jellyfin_md5("PASSWORD-RESET-USER"),
            "ee941ebf0999c2c0a333d9d14c13fb6b"
        );
    }
}
