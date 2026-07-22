use jellyfin_data::entities::user;
use jellyfin_model::{PasswordHash, PasswordHashError};
use thiserror::Error;

use crate::{CryptographyError, CryptographyProvider};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAuthenticationResult {
    pub username: String,
    pub password_hash_upgraded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AuthenticationError {
    #[error("invalid username or password")]
    InvalidCredentials,
    #[error(transparent)]
    InvalidPasswordHash(#[from] PasswordHashError),
    #[error(transparent)]
    Cryptography(#[from] CryptographyError),
}

/// Jellyfin's built-in local-user authentication provider.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultAuthenticationProvider {
    cryptography: CryptographyProvider,
}

impl DefaultAuthenticationProvider {
    pub const NAME: &'static str = "Default";

    #[must_use]
    pub const fn new() -> Self {
        Self {
            cryptography: CryptographyProvider::new(),
        }
    }

    #[must_use]
    pub const fn name(&self) -> &'static str {
        Self::NAME
    }

    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        true
    }

    /// Authenticates a resolved local user and upgrades an obsolete password
    /// hash in the supplied `SeaORM` model after successful verification.
    ///
    /// # Errors
    ///
    /// Returns [`AuthenticationError::InvalidCredentials`] for a missing user
    /// or wrong password. Malformed and unsupported stored hashes are reported
    /// separately so operators can repair corrupted authentication data.
    pub fn authenticate(
        &self,
        username: &str,
        password: &str,
        resolved_user: Option<&mut user::Model>,
    ) -> Result<ProviderAuthenticationResult, AuthenticationError> {
        let user = resolved_user.ok_or(AuthenticationError::InvalidCredentials)?;

        if user.password_hash.as_deref().is_none_or(str::is_empty) && password.is_empty() {
            return Ok(ProviderAuthenticationResult {
                username: username.to_owned(),
                password_hash_upgraded: false,
            });
        }

        let stored_hash = user
            .password_hash
            .as_deref()
            .ok_or(AuthenticationError::InvalidCredentials)?;
        let password_hash = PasswordHash::parse(stored_hash)?;
        if !self.cryptography.verify(&password_hash, password)? {
            return Err(AuthenticationError::InvalidCredentials);
        }

        let password_hash_upgraded = self.cryptography.needs_rehash(&password_hash)?;
        if password_hash_upgraded {
            self.change_password(user, password);
        }

        Ok(ProviderAuthenticationResult {
            username: username.to_owned(),
            password_hash_upgraded,
        })
    }

    pub fn change_password(&self, user: &mut user::Model, new_password: &str) {
        user.password_hash = if new_password.is_empty() {
            None
        } else {
            Some(
                self.cryptography
                    .create_password_hash(new_password)
                    .to_string(),
            )
        };
    }
}
