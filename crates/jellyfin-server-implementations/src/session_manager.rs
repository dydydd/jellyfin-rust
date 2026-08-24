use std::{error::Error, fmt, future::Future, pin::Pin};

use thiserror::Error;
use uuid::Uuid;

/// An asynchronous result returned by a [`SessionStore`] operation.
pub type SessionStoreFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

/// Client metadata supplied when authenticating a new session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthenticationRequest {
    pub app: Option<String>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub app_version: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl AuthenticationRequest {
    /// Creates a request with all metadata fields present.
    #[must_use]
    pub fn new(
        app: impl Into<String>,
        device_id: impl Into<String>,
        device_name: impl Into<String>,
        app_version: impl Into<String>,
    ) -> Self {
        Self {
            app: Some(app.into()),
            device_id: Some(device_id.into()),
            device_name: Some(device_name.into()),
            app_version: Some(app_version.into()),
            username: None,
            password: None,
        }
    }

    /// Creates a request with all client metadata and local credentials.
    #[must_use]
    pub fn with_credentials(
        app: impl Into<String>,
        device_id: impl Into<String>,
        device_name: impl Into<String>,
        app_version: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            app: Some(app.into()),
            device_id: Some(device_id.into()),
            device_name: Some(device_name.into()),
            app_version: Some(app_version.into()),
            username: Some(username.into()),
            password: Some(password.into()),
        }
    }
}

/// A request whose required session metadata has passed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedAuthenticationRequest {
    app: String,
    device_id: String,
    device_name: String,
    app_version: String,
    username: Option<String>,
    password: Option<String>,
}

impl ValidatedAuthenticationRequest {
    #[must_use]
    pub fn app(&self) -> &str {
        &self.app
    }

    #[must_use]
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    #[must_use]
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    #[must_use]
    pub fn app_version(&self) -> &str {
        &self.app_version
    }

    #[must_use]
    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    #[must_use]
    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }
}

/// Validated metadata required to create a device authorization token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationTokenRequest {
    pub user_id: Uuid,
    pub device_id: String,
    pub app: String,
    pub app_version: String,
    pub device_name: String,
}

/// Persistence and authentication boundary used after input validation.
///
/// Implementations own user authentication, token replacement, device
/// persistence, and the resulting session representation. `SessionManager`
/// deliberately does not claim to implement those lifecycle operations.
pub trait SessionStore {
    type AuthenticationResult;
    type Error: Error + Send + Sync + 'static;

    fn authenticate_new_session(
        &self,
        request: ValidatedAuthenticationRequest,
        enforce_password: bool,
    ) -> SessionStoreFuture<'_, Self::AuthenticationResult, Self::Error>;

    fn issue_authorization_token(
        &self,
        request: AuthorizationTokenRequest,
    ) -> SessionStoreFuture<'_, String, Self::Error>;
}

/// Required authentication metadata fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticationField {
    App,
    DeviceId,
    DeviceName,
    AppVersion,
}

impl fmt::Display for AuthenticationField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::App => "app",
            Self::DeviceId => "device id",
            Self::DeviceName => "device name",
            Self::AppVersion => "app version",
        })
    }
}

/// Input validation failures raised before the session store is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SessionValidationError {
    #[error("required authentication field {0} is missing")]
    MissingField(AuthenticationField),
    #[error("required authentication field {0} is empty")]
    EmptyField(AuthenticationField),
}

/// Session orchestration failure.
#[derive(Debug, PartialEq, Eq, Error)]
pub enum SessionManagerError<E: Error + 'static> {
    #[error(transparent)]
    Validation(#[from] SessionValidationError),
    #[error("session store failed: {0}")]
    Store(E),
}

/// Validates session inputs and delegates lifecycle work to a [`SessionStore`].
#[derive(Debug, Clone)]
pub struct SessionManager<S> {
    store: S,
}

impl<S> SessionManager<S> {
    #[must_use]
    pub const fn new(store: S) -> Self {
        Self { store }
    }

    #[must_use]
    pub const fn store(&self) -> &S {
        &self.store
    }
}

impl<S: SessionStore> SessionManager<S> {
    /// Validates a new-session request in Jellyfin's official field order and
    /// delegates authentication and persistence to the session store.
    ///
    /// # Errors
    ///
    /// Returns distinct missing/empty validation errors without calling the
    /// store, or wraps the store's authentication error unchanged.
    pub async fn authenticate_new_session_internal(
        &self,
        request: &AuthenticationRequest,
        enforce_password: bool,
    ) -> Result<S::AuthenticationResult, SessionManagerError<S::Error>> {
        let validated = validate_authentication_request(request)?;
        self.store
            .authenticate_new_session(validated, enforce_password)
            .await
            .map_err(SessionManagerError::Store)
    }

    /// Validates the device identifier and delegates authorization-token
    /// replacement and creation to the session store.
    ///
    /// # Errors
    ///
    /// Returns distinct missing/empty device-id errors without calling the
    /// store, or wraps the store error unchanged.
    pub async fn get_authorization_token(
        &self,
        user_id: Uuid,
        device_id: Option<String>,
        app: String,
        app_version: String,
        device_name: String,
    ) -> Result<String, SessionManagerError<S::Error>> {
        let device_id = validate_required(device_id.as_ref(), AuthenticationField::DeviceId)?;
        self.store
            .issue_authorization_token(AuthorizationTokenRequest {
                user_id,
                device_id,
                app,
                app_version,
                device_name,
            })
            .await
            .map_err(SessionManagerError::Store)
    }
}

fn validate_authentication_request(
    request: &AuthenticationRequest,
) -> Result<ValidatedAuthenticationRequest, SessionValidationError> {
    Ok(ValidatedAuthenticationRequest {
        app: validate_required(request.app.as_ref(), AuthenticationField::App)?,
        device_id: validate_required(request.device_id.as_ref(), AuthenticationField::DeviceId)?,
        device_name: validate_required(
            request.device_name.as_ref(),
            AuthenticationField::DeviceName,
        )?,
        app_version: validate_required(
            request.app_version.as_ref(),
            AuthenticationField::AppVersion,
        )?,
        username: request.username.clone(),
        password: request.password.clone(),
    })
}

fn validate_required(
    value: Option<&String>,
    field: AuthenticationField,
) -> Result<String, SessionValidationError> {
    match value {
        None => Err(SessionValidationError::MissingField(field)),
        Some(value) if value.is_empty() => Err(SessionValidationError::EmptyField(field)),
        Some(value) => Ok(value.clone()),
    }
}
