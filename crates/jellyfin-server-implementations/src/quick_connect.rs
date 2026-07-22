use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Duration, Utc};
use jellyfin_data::{
    NewQuickConnectRequest, QuickConnectRepository, QuickConnectStoreError,
    entities::{device, quick_connect},
};
use rand::{Rng, RngCore};
use thiserror::Error;
use uuid::Uuid;

const REQUEST_TIMEOUT_MINUTES: i64 = 10;
const TOKEN_GENERATION_ATTEMPTS: usize = 32;

/// Client and device metadata attached to a Quick Connect request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizationInfo {
    pub device_name: String,
    pub device_id: String,
    pub app_name: String,
    pub app_version: String,
}

/// Public Quick Connect request state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuickConnectResult {
    pub secret: String,
    pub code: String,
    pub date_added: DateTime<Utc>,
    pub device_id: String,
    pub device_name: String,
    pub app_name: String,
    pub app_version: String,
    pub authenticated: bool,
}

impl From<quick_connect::Model> for QuickConnectResult {
    fn from(model: quick_connect::Model) -> Self {
        Self {
            secret: model.secret,
            code: model.code,
            date_added: model.created_at,
            device_id: model.device_id,
            device_name: model.device_name,
            app_name: model.app_name,
            app_version: model.app_version,
            authenticated: model.authorized_at.is_some(),
        }
    }
}

/// Authentication response created when a Quick Connect request is authorized.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuickConnectAuthenticationResult {
    pub user_id: Uuid,
    pub access_token: String,
    pub device_id: String,
    pub device_name: String,
    pub app_name: String,
    pub app_version: String,
}

impl From<device::Model> for QuickConnectAuthenticationResult {
    fn from(model: device::Model) -> Self {
        Self {
            user_id: model.user_id,
            access_token: model.access_token,
            device_id: model.device_id,
            device_name: model.device_name,
            app_name: model.app_name,
            app_version: model.app_version,
        }
    }
}

/// Runtime boundary for enablement, time, and secure token generation.
pub trait QuickConnectCapability {
    fn is_enabled(&self) -> bool;

    fn now(&self) -> DateTime<Utc>;

    fn generate_code(&self) -> String;

    fn generate_secret(&self) -> String;
}

/// Default runtime capability backed by the system clock and cryptographic RNG.
#[derive(Clone, Debug)]
pub struct SystemQuickConnectCapability {
    enabled: Arc<AtomicBool>,
}

impl SystemQuickConnectCapability {
    #[must_use]
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(enabled)),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }
}

impl QuickConnectCapability for SystemQuickConnectCapability {
    fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn generate_code(&self) -> String {
        rand::rng().random_range(100_000..1_000_000).to_string()
    }

    fn generate_secret(&self) -> String {
        let mut bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        encode_upper_hex(&bytes)
    }
}

/// Quick Connect lifecycle failure.
#[derive(Debug, Error)]
pub enum QuickConnectError {
    #[error("Quick Connect authorization {0} cannot be empty")]
    InvalidAuthorization(&'static str),
    #[error("Quick Connect is not active on this server")]
    Disabled,
    #[error("Quick Connect request was not found or has expired")]
    NotFound,
    #[error("Quick Connect request is already authorized")]
    AlreadyAuthorized,
    #[error("unable to generate a unique Quick Connect code and secret")]
    TokenGenerationExhausted,
    #[error(transparent)]
    Store(QuickConnectStoreError),
}

/// PostgreSQL-backed Quick Connect manager.
#[derive(Clone)]
pub struct QuickConnectManager<C> {
    repository: QuickConnectRepository,
    capability: C,
}

impl<C: QuickConnectCapability> QuickConnectManager<C> {
    #[must_use]
    pub const fn new(repository: QuickConnectRepository, capability: C) -> Self {
        Self {
            repository,
            capability,
        }
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.capability.is_enabled()
    }

    /// Starts a pending Quick Connect request.
    ///
    /// # Errors
    ///
    /// Validates authorization metadata before enablement, then returns a
    /// disabled, token-exhaustion, validation, or persistence error.
    pub async fn try_connect(
        &self,
        authorization: &AuthorizationInfo,
    ) -> Result<QuickConnectResult, QuickConnectError> {
        validate_authorization(authorization)?;
        self.assert_active()?;
        let now = self.capability.now();
        self.repository
            .purge_expired(now)
            .await
            .map_err(map_store_error)?;
        for _ in 0..TOKEN_GENERATION_ATTEMPTS {
            let request = NewQuickConnectRequest {
                code: self.capability.generate_code(),
                secret: self.capability.generate_secret(),
                device_id: authorization.device_id.clone(),
                device_name: authorization.device_name.clone(),
                app_name: authorization.app_name.clone(),
                app_version: authorization.app_version.clone(),
                created_at: now,
                expires_at: now + Duration::minutes(REQUEST_TIMEOUT_MINUTES),
            };
            match self.repository.create(request).await {
                Ok(request) => return Ok(request.into()),
                Err(QuickConnectStoreError::Conflict) => {}
                Err(error) => return Err(map_store_error(error)),
            }
        }
        Err(QuickConnectError::TokenGenerationExhausted)
    }

    /// Returns the current state for an unexpired secret.
    ///
    /// # Errors
    ///
    /// Returns disabled, not-found, or persistence errors.
    pub async fn check_request_status(
        &self,
        secret: &str,
    ) -> Result<QuickConnectResult, QuickConnectError> {
        self.assert_active()?;
        self.repository
            .status(secret, self.capability.now())
            .await
            .map_err(map_store_error)?
            .map(QuickConnectResult::from)
            .ok_or(QuickConnectError::NotFound)
    }

    /// Authorizes a pending code for `user_id` and creates an active device session.
    ///
    /// # Errors
    ///
    /// Returns disabled, unknown/expired, duplicate authorization, user/device,
    /// or persistence errors.
    pub async fn authorize_request(
        &self,
        user_id: Uuid,
        code: &str,
    ) -> Result<bool, QuickConnectError> {
        self.assert_active()?;
        let now = self.capability.now();
        self.repository
            .authorize(
                code,
                user_id,
                now,
                now + Duration::minutes(REQUEST_TIMEOUT_MINUTES),
            )
            .await
            .map_err(map_store_error)?;
        Ok(true)
    }

    /// Returns the device authentication created for an authorized secret.
    ///
    /// # Errors
    ///
    /// Returns disabled, not-found/expired, or persistence errors.
    pub async fn get_authorized_request(
        &self,
        secret: &str,
    ) -> Result<QuickConnectAuthenticationResult, QuickConnectError> {
        self.assert_active()?;
        self.repository
            .authorized(secret, self.capability.now())
            .await
            .map(|authorized| authorized.device.into())
            .map_err(map_store_error)
    }

    fn assert_active(&self) -> Result<(), QuickConnectError> {
        if self.is_enabled() {
            Ok(())
        } else {
            Err(QuickConnectError::Disabled)
        }
    }
}

fn validate_authorization(authorization: &AuthorizationInfo) -> Result<(), QuickConnectError> {
    for (field, value) in [
        ("device name", authorization.device_name.as_str()),
        ("device id", authorization.device_id.as_str()),
        ("app name", authorization.app_name.as_str()),
        ("app version", authorization.app_version.as_str()),
    ] {
        if value.is_empty() {
            return Err(QuickConnectError::InvalidAuthorization(field));
        }
    }
    Ok(())
}

fn map_store_error(error: QuickConnectStoreError) -> QuickConnectError {
    match error {
        QuickConnectStoreError::NotFound => QuickConnectError::NotFound,
        QuickConnectStoreError::AlreadyAuthorized => QuickConnectError::AlreadyAuthorized,
        error => QuickConnectError::Store(error),
    }
}

fn encode_upper_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}
