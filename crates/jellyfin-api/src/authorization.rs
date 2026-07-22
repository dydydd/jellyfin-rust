use axum::http::{HeaderMap, Uri};
use chrono::Local;

use crate::{
    ApiError, AppState,
    authentication::{self, AuthenticatedIdentity},
};

/// Applies Jellyfin's default authenticated-user policy, including parental schedules.
pub(crate) async fn require_default(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<AuthenticatedIdentity, ApiError> {
    let identity = authentication::authenticated_identity(state, headers, Some(uri)).await?;
    identity.require_parental_schedule(Local::now().fixed_offset())?;
    Ok(identity)
}

/// Authenticates while deliberately bypassing parental schedules.
pub(crate) async fn require_ignore_parental_control(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<AuthenticatedIdentity, ApiError> {
    authentication::authenticated_identity(state, headers, Some(uri)).await
}

/// Applies Jellyfin's startup-wizard-or-elevated authorization policy.
pub(crate) async fn require_first_time_setup_or_elevated(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<(), ApiError> {
    let startup_completed = crate::startup::is_completed(state).await?;
    if !startup_completed {
        return Ok(());
    }

    authentication::authenticated_identity(state, headers, Some(uri))
        .await?
        .require_administrator()
}
