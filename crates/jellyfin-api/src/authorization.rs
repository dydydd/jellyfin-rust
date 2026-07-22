use axum::http::{HeaderMap, Uri};

use crate::{ApiError, AppState, authentication};

/// Applies Jellyfin's startup-wizard-or-elevated authorization policy.
pub(crate) async fn require_first_time_setup_or_elevated(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<(), ApiError> {
    let startup_completed = state.startup.lock().await.completed;
    if !startup_completed {
        return Ok(());
    }

    authentication::authenticated_identity(state, headers, Some(uri))
        .await?
        .require_administrator()
}
