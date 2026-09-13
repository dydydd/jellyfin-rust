//! Emby typed user settings.
//!
//! The generated clients model each setting as an opaque binary stream. Store
//! those bytes in a protocol-owned display-preference row so values survive a
//! restart without introducing the route or its wire shape into Jellyfin.

use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::{OriginalUri, Path, State, rejection::BytesRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use jellyfin_api::AppState;
use jellyfin_model::DisplayPreferencesDto;

const DISPLAY_PREFERENCES_ID_PREFIX: &str = "emby-typed-setting:";
const DISPLAY_PREFERENCES_CLIENT: &str = "Emby.TypedSettings";
const CUSTOM_PREFERENCE_KEY: &str = "payloadBase64";

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Users/{user_id}/TypedSettings/{key}", get(load).post(save))
        .route("/users/{user_id}/typedsettings/{key}", get(load).post(save))
}

async fn load(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((user_id, key)): Path<(String, String)>,
) -> Result<Response, Response> {
    let preferences = preferences(&state, &headers, &uri, &user_id, &key).await?;
    let payload = preferences
        .custom_prefs
        .get(CUSTOM_PREFERENCE_KEY)
        .and_then(Option::as_deref)
        .map(|encoded| BASE64_STANDARD.decode(encoded))
        .transpose()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?
        .unwrap_or_default();
    Ok((
        [(header::CONTENT_TYPE, "application/octet-stream")],
        payload,
    )
        .into_response())
}

async fn save(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((user_id, key)): Path<(String, String)>,
    payload: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, Response> {
    // Resolve and authorize the target before surfacing body extraction
    // failures, matching the target-precedence used by the user APIs.
    let mut preferences = preferences(&state, &headers, &uri, &user_id, &key).await?;
    let payload = payload.map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    preferences.custom_prefs.insert(
        CUSTOM_PREFERENCE_KEY.to_owned(),
        Some(BASE64_STANDARD.encode(payload)),
    );
    state
        .update_display_preferences_for_request(
            &headers,
            &uri,
            &display_preferences_id(&key),
            Some(&user_id),
            None,
            Some(DISPLAY_PREFERENCES_CLIENT.to_owned()),
            preferences,
        )
        .await?;
    Ok(StatusCode::OK)
}

async fn preferences(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
    user_id: &str,
    key: &str,
) -> Result<DisplayPreferencesDto, Response> {
    state
        .display_preferences_for_request(
            headers,
            uri,
            &display_preferences_id(key),
            Some(user_id),
            None,
            Some(DISPLAY_PREFERENCES_CLIENT.to_owned()),
        )
        .await
}

fn display_preferences_id(key: &str) -> String {
    format!("{DISPLAY_PREFERENCES_ID_PREFIX}{key}")
}
