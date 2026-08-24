use std::collections::hash_map::Entry;
use std::sync::Arc;

use axum::{
    Json,
    extract::{
        OriginalUri, Path, Query, State, rejection::JsonRejection, rejection::QueryRejection,
    },
    http::{HeaderMap, StatusCode},
};
use jellyfin_model::DisplayPreferencesDto;
use md5::{Digest, Md5};
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct DisplayPreferencesQuery {
    #[serde(rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(rename = "itemId", alias = "ItemId")]
    item_id: Option<Uuid>,
    #[serde(rename = "client", alias = "Client")]
    client: Option<String>,
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(display_preferences_id): Path<String>,
    query: Result<Query<DisplayPreferencesQuery>, QueryRejection>,
) -> Result<Json<DisplayPreferencesDto>, ApiError> {
    let (target_user_id, client, item_id) =
        target_user_and_client(&state, &headers, &uri, &display_preferences_id, query).await?;
    let preferences = state
        .display_preferences
        .find(target_user_id, item_id, &client)
        .await?;
    Ok(Json(display_preferences_dto(
        item_id,
        &client,
        preferences.map(|preferences| preferences.preferences),
    )?))
}

pub(crate) async fn update(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(display_preferences_id): Path<String>,
    query: Result<Query<DisplayPreferencesQuery>, QueryRejection>,
    request: Result<Json<DisplayPreferencesDto>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let (target_user_id, client, item_id) =
        target_user_and_client(&state, &headers, &uri, &display_preferences_id, query).await?;
    let Json(mut preferences) = request.map_err(|_| ApiError::InvalidRequest)?;
    preferences.id = Some(item_id.to_string());
    preferences.client = Some(client.clone());
    normalize_official_defaults(&mut preferences);
    let preferences = serde_json::to_value(preferences).map_err(|_| ApiError::Internal)?;
    state
        .display_preferences
        .upsert(target_user_id, item_id, &client, preferences)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn target_user_and_client(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
    display_preferences_id: &str,
    query: Result<Query<DisplayPreferencesQuery>, QueryRejection>,
) -> Result<(Uuid, String, Uuid), ApiError> {
    let identity = authentication::authenticated_identity(state, headers, Some(uri)).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let user_id = identity.target_user_id(query.user_id)?;
    if user_id.is_nil() {
        return Err(ApiError::InvalidRequest);
    }
    state.users.get(user_id).await?;
    let client = query
        .client
        .filter(|client| !client.trim().is_empty())
        .ok_or(ApiError::InvalidRequest)?;
    let item_id = query
        .item_id
        .unwrap_or_else(|| display_preferences_item_id(display_preferences_id));
    Ok((user_id, client, item_id))
}

fn display_preferences_dto(
    item_id: Uuid,
    client: &str,
    preferences: Option<serde_json::Value>,
) -> Result<DisplayPreferencesDto, ApiError> {
    let mut preferences: DisplayPreferencesDto = preferences
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| ApiError::Internal)?
        .unwrap_or_default();
    preferences.id = Some(item_id.to_string());
    preferences.client = Some(client.to_owned());
    normalize_official_defaults(&mut preferences);
    Ok(preferences)
}

fn normalize_official_defaults(preferences: &mut DisplayPreferencesDto) {
    preferences
        .sort_by
        .get_or_insert_with(|| "SortName".to_owned());
    for (key, value) in [
        ("chromecastVersion", "stable"),
        ("skipForwardLength", "15000"),
        ("skipBackLength", "15000"),
        ("enableNextVideoInfoOverlay", "true"),
        ("tvhome", ""),
        ("dashboardTheme", ""),
    ] {
        match preferences.custom_prefs.entry(key.to_owned()) {
            Entry::Occupied(_) => {}
            Entry::Vacant(entry) => {
                entry.insert(Some(value.to_owned()));
            }
        }
    }
}

fn display_preferences_item_id(display_preferences_id: &str) -> Uuid {
    Uuid::parse_str(display_preferences_id)
        .unwrap_or_else(|_| jellyfin_md5_guid(display_preferences_id))
}

fn jellyfin_md5_guid(value: &str) -> Uuid {
    let mut hasher = Md5::new();
    for unit in value.encode_utf16() {
        hasher.update(unit.to_le_bytes());
    }
    Uuid::from_bytes_le(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::display_preferences_item_id;

    #[test]
    fn non_guid_display_preferences_ids_match_jellyfin_md5_guid() {
        assert_eq!(
            display_preferences_item_id("usersettings").to_string(),
            "3ce5b65d-e116-d731-65d1-efc4a30ec35c"
        );
    }
}
