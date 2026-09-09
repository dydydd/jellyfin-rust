//! Wire-shape adapters for Emby's authentication/user bootstrap APIs.
//!
//! Most authentication, user, device, and session DTOs are shared with
//! Jellyfin.  Display preferences are the exception: Emby's generated
//! Android/iOS clients model `CustomPrefs` as a non-null string map, while
//! Jellyfin permits null values internally.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{OriginalUri, Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use jellyfin_api::AppState;
use jellyfin_model::{DisplayPreferencesDto, SortOrder};
use serde::{Deserialize, Serialize};

/// Emby display-preference paths share persistence with Jellyfin while
/// retaining Emby's narrower DTO.
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/DisplayPreferences/{display_preferences_id}",
            get(get_display_preferences).post(update_display_preferences),
        )
        .route(
            "/displaypreferences/{display_preferences_id}",
            get(get_display_preferences).post(update_display_preferences),
        )
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct DisplayPreferencesQuery {
    #[serde(rename = "userId", alias = "UserId")]
    user_id: Option<String>,
    #[serde(rename = "itemId", alias = "ItemId")]
    item_id: Option<String>,
    #[serde(rename = "client", alias = "Client")]
    client: Option<String>,
}

/// Emby's `DisplayPreferences` wire contract used by local Android/iOS SDKs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct EmbyDisplayPreferences {
    #[serde(rename = "Id")]
    pub id: Option<String>,
    pub sort_by: Option<String>,
    pub custom_prefs: HashMap<String, String>,
    pub sort_order: SortOrder,
    pub client: Option<String>,
}

impl From<DisplayPreferencesDto> for EmbyDisplayPreferences {
    fn from(value: DisplayPreferencesDto) -> Self {
        Self {
            id: value.id,
            sort_by: value.sort_by,
            custom_prefs: value
                .custom_prefs
                .into_iter()
                .filter_map(|(key, value)| value.map(|value| (key, value)))
                .collect(),
            sort_order: value.sort_order,
            client: value.client,
        }
    }
}

impl From<EmbyDisplayPreferences> for DisplayPreferencesDto {
    fn from(value: EmbyDisplayPreferences) -> Self {
        Self {
            id: value.id,
            sort_by: value.sort_by,
            custom_prefs: value
                .custom_prefs
                .into_iter()
                .map(|(key, value)| (key, Some(value)))
                .collect(),
            sort_order: value.sort_order,
            client: value.client,
            ..Default::default()
        }
    }
}

/// Keep only fields represented by Emby's generated `DisplayPreferences`
/// model when serializing a response.  This avoids exposing Jellyfin-only
/// fields that older Emby clients do not model.
pub fn display_preferences_response(preferences: DisplayPreferencesDto) -> EmbyDisplayPreferences {
    preferences.into()
}

/// Parse an Emby request and expand its compact DTO into the internal model.
pub fn display_preferences_request(preferences: EmbyDisplayPreferences) -> DisplayPreferencesDto {
    preferences.into()
}

async fn get_display_preferences(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(display_preferences_id): Path<String>,
    Query(query): Query<DisplayPreferencesQuery>,
) -> Result<Json<EmbyDisplayPreferences>, Response> {
    let preferences = state
        .display_preferences_for_request(
            &headers,
            &uri,
            &display_preferences_id,
            query.user_id.as_deref(),
            query.item_id.as_deref(),
            query.client,
        )
        .await?;
    Ok(Json(display_preferences_response(preferences)))
}

async fn update_display_preferences(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(display_preferences_id): Path<String>,
    Query(query): Query<DisplayPreferencesQuery>,
    request: Result<Json<EmbyDisplayPreferences>, JsonRejection>,
) -> Result<StatusCode, Response> {
    let Json(preferences) = request.map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    state
        .update_display_preferences_for_request(
            &headers,
            &uri,
            &display_preferences_id,
            query.user_id.as_deref(),
            query.item_id.as_deref(),
            query.client,
            display_preferences_request(preferences),
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn emby_custom_prefs_never_serializes_null_values_or_jellyfin_fields() {
        let mut value = DisplayPreferencesDto::default();
        value.id = Some("usersettings".to_owned());
        value.view_type = Some("PosterCard".to_owned());
        value.custom_prefs.insert("null".to_owned(), None);
        value
            .custom_prefs
            .insert("theme".to_owned(), Some("dark".to_owned()));

        let wire = serde_json::to_value(display_preferences_response(value)).unwrap();
        assert_eq!(wire["CustomPrefs"], json!({"theme": "dark"}));
        assert!(wire.get("ViewType").is_none());
        assert!(wire.get("RememberIndexing").is_none());
    }

    #[test]
    fn emby_custom_prefs_round_trip_as_strings() {
        let input = json!({
            "Id": "abc",
            "SortBy": "SortName",
            "CustomPrefs": {"tvhome": "", "skipForwardLength": "15000"},
            "SortOrder": "Descending",
            "Client": "Emby"
        });
        let emby: EmbyDisplayPreferences = serde_json::from_value(input).unwrap();
        let internal = display_preferences_request(emby.clone());
        assert_eq!(internal.custom_prefs["tvhome"].as_deref(), Some(""));
        assert_eq!(display_preferences_response(internal), emby);
    }
}
