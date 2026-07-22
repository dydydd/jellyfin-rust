use jellyfin_model::{PasswordHash, PasswordHashError};
use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use sha1::Sha1;
use sha2::Sha512;
use subtle::ConstantTimeEq;
use thiserror::Error;

pub const DEFAULT_HASH_METHOD: &str = "PBKDF2-SHA512";
pub const DEFAULT_SALT_LENGTH: usize = 128 / 8;
pub const DEFAULT_OUTPUT_LENGTH: usize = 512 / 8;
pub const DEFAULT_ITERATIONS: u32 = 210_000;
const LEGACY_OUTPUT_LENGTH: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CryptographyError {
    #[error("password hash with id '{0}' is missing required 'iterations' parameter")]
    MissingIterations(String),
    #[error("password hash with id '{method}' has invalid 'iterations' parameter: '{value}'")]
    InvalidIterations { method: String, value: String },
    #[error("can't verify hash with id: {0}")]
    UnsupportedHashMethod(String),
    #[error(transparent)]
    PasswordHash(#[from] PasswordHashError),
}

/// Jellyfin-compatible password hashing and verification.
#[derive(Debug, Clone, Copy, Default)]
pub struct CryptographyProvider;

impl CryptographyProvider {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    #[must_use]
    pub const fn default_hash_method(&self) -> &'static str {
        DEFAULT_HASH_METHOD
    }

    #[must_use]
    /// Creates a password hash using Jellyfin's current defaults.
    ///
    /// # Panics
    ///
    /// Panics only if the built-in nonempty hash method constant becomes
    /// invalid for [`PasswordHash`].
    pub fn create_password_hash(&self, password: &str) -> PasswordHash {
        let salt = self.generate_salt(DEFAULT_SALT_LENGTH);
        let mut hash = vec![0; DEFAULT_OUTPUT_LENGTH];
        pbkdf2_hmac::<Sha512>(password.as_bytes(), &salt, DEFAULT_ITERATIONS, &mut hash);
        let parameters = [("iterations".to_owned(), DEFAULT_ITERATIONS.to_string())]
            .into_iter()
            .collect();
        PasswordHash::with_parameters(DEFAULT_HASH_METHOD, hash, salt, parameters)
            .expect("the built-in password hash method is valid")
    }

    /// Verifies `password` against either Jellyfin's legacy SHA-1 PBKDF2
    /// format or its current SHA-512 PBKDF2 format.
    ///
    /// # Errors
    ///
    /// Returns an error when the iteration parameter is missing or invalid,
    /// or when the hash method is unsupported.
    pub fn verify(&self, hash: &PasswordHash, password: &str) -> Result<bool, CryptographyError> {
        let derived = match hash.id() {
            "PBKDF2" => {
                let iterations = iterations(hash)?;
                let mut derived = vec![0; LEGACY_OUTPUT_LENGTH];
                pbkdf2_hmac::<Sha1>(password.as_bytes(), hash.salt(), iterations, &mut derived);
                derived
            }
            DEFAULT_HASH_METHOD => {
                let iterations = iterations(hash)?;
                let mut derived = vec![0; DEFAULT_OUTPUT_LENGTH];
                pbkdf2_hmac::<Sha512>(password.as_bytes(), hash.salt(), iterations, &mut derived);
                derived
            }
            method => return Err(CryptographyError::UnsupportedHashMethod(method.to_owned())),
        };

        Ok(bool::from(hash.hash().ct_eq(derived.as_slice())))
    }

    /// Determines whether a verified hash should be migrated to Jellyfin's
    /// current method and work factor.
    ///
    /// # Errors
    ///
    /// Returns an error when the iteration parameter is missing or invalid.
    pub fn needs_rehash(&self, hash: &PasswordHash) -> Result<bool, CryptographyError> {
        if hash.id() != DEFAULT_HASH_METHOD {
            return Ok(true);
        }
        Ok(iterations(hash)? != DEFAULT_ITERATIONS)
    }

    #[must_use]
    pub fn generate_salt(&self, length: usize) -> Vec<u8> {
        let mut salt = vec![0; length];
        let mut rng = rand::rng();
        rng.fill_bytes(&mut salt);
        for byte in &mut salt {
            while *byte == 0 {
                rng.fill_bytes(std::slice::from_mut(byte));
            }
        }
        salt
    }
}

fn iterations(hash: &PasswordHash) -> Result<u32, CryptographyError> {
    let value = hash
        .parameters()
        .get("iterations")
        .ok_or_else(|| CryptographyError::MissingIterations(hash.id().to_owned()))?;
    value
        .parse::<u32>()
        .ok()
        .filter(|iterations| *iterations > 0)
        .ok_or_else(|| CryptographyError::InvalidIterations {
            method: hash.id().to_owned(),
            value: value.clone(),
        })
}
