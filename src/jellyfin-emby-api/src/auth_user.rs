//! Wire-shape adapters for Emby's authentication/user bootstrap APIs.
//!
//! Most authentication, user, device, and session DTOs are shared with
//! Jellyfin.  Display preferences are the exception: Emby's generated
//! Android/iOS clients model `CustomPrefs` as a non-null string map, while
//! Jellyfin permits null values internally.

use std::{collections::HashMap, fmt, sync::Arc};

use axum::{
    Json, Router,
    extract::{OriginalUri, Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use jellyfin_api::AppState;
use jellyfin_model::{DisplayPreferencesDto, SortOrder};
use serde::{Deserialize, Serialize, de};

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
        .route(
            "/UserSettings/{user_id}",
            get(get_user_settings).post(update_user_settings),
        )
        .route(
            "/usersettings/{user_id}",
            get(get_user_settings).post(update_user_settings),
        )
        .route(
            "/UserSettings/{user_id}/Partial",
            axum::routing::post(partial_user_settings),
        )
        .route(
            "/usersettings/{user_id}/partial",
            axum::routing::post(partial_user_settings),
        )
}

const USER_SETTINGS_CLIENT: &str = "Emby";

async fn get_user_settings(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> Result<Json<HashMap<String, String>>, Response> {
    let preferences = state
        .display_preferences_for_request(
            &headers,
            &uri,
            "usersettings",
            Some(&user_id),
            None,
            Some(USER_SETTINGS_CLIENT.to_owned()),
        )
        .await?;
    Ok(Json(
        preferences
            .custom_prefs
            .into_iter()
            .filter_map(|(key, value)| value.map(|value| (key, value)))
            .collect(),
    ))
}

async fn update_user_settings(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    body: axum::body::Bytes,
) -> Result<StatusCode, Response> {
    let settings =
        parse_settings_body(&body).map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    let mut preferences = jellyfin_model::DisplayPreferencesDto::default();
    preferences.custom_prefs = settings;
    state
        .update_display_preferences_for_request(
            &headers,
            &uri,
            "usersettings",
            Some(&user_id),
            None,
            Some(USER_SETTINGS_CLIENT.to_owned()),
            preferences,
        )
        .await?;
    Ok(StatusCode::OK)
}

fn parse_user_settings(settings: Vec<String>) -> Result<HashMap<String, Option<String>>, ()> {
    settings
        .into_iter()
        .map(|setting| {
            let (key, value) = setting.split_once('=').ok_or(())?;
            (!key.is_empty())
                .then(|| (key.to_owned(), Some(value.to_owned())))
                .ok_or(())
        })
        .collect()
}

fn parse_settings_body(body: &[u8]) -> Result<HashMap<String, Option<String>>, ()> {
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|_| ())?;
    match value {
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(|value| value.as_str().map(str::to_owned).ok_or(()))
            .collect::<Result<Vec<_>, _>>()
            .and_then(parse_user_settings),
        serde_json::Value::Object(values) => values
            .into_iter()
            .map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key, Some(value.to_owned())))
                    .ok_or(())
            })
            .collect(),
        _ => Err(()),
    }
}

async fn partial_user_settings(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    body: axum::body::Bytes,
) -> Result<StatusCode, Response> {
    let settings =
        parse_settings_body(&body).map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    let mut preferences = state
        .display_preferences_for_request(
            &headers,
            &uri,
            "usersettings",
            Some(&user_id),
            None,
            Some(USER_SETTINGS_CLIENT.to_owned()),
        )
        .await?;
    preferences.custom_prefs.extend(settings);
    state
        .update_display_preferences_for_request(
            &headers,
            &uri,
            "usersettings",
            Some(&user_id),
            None,
            Some(USER_SETTINGS_CLIENT.to_owned()),
            preferences,
        )
        .await?;
    Ok(StatusCode::OK)
}

#[derive(Debug, Default)]
struct DisplayPreferencesQuery {
    user_id: Option<String>,
    item_id: Option<String>,
    client: Option<String>,
}

impl<'de> Deserialize<'de> for DisplayPreferencesQuery {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = DisplayPreferencesQuery;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby display-preferences query")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut query = DisplayPreferencesQuery::default();
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("UserId") {
                        query.user_id = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("ItemId") {
                        query.item_id = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("Client") {
                        query.client = map.next_value()?;
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(query)
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

/// Emby's `DisplayPreferences` wire contract used by local Android/iOS SDKs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct EmbyDisplayPreferences {
    #[serde(rename = "Id")]
    pub id: Option<String>,
    pub sort_by: Option<String>,
    pub custom_prefs: HashMap<String, String>,
    pub sort_order: SortOrder,
    pub client: Option<String>,
}

impl<'de> Deserialize<'de> for EmbyDisplayPreferences {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = EmbyDisplayPreferences;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby DisplayPreferences object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut preferences = EmbyDisplayPreferences::default();
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("Id") {
                        preferences.id = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("SortBy") {
                        preferences.sort_by = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("CustomPrefs") {
                        preferences.custom_prefs = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("SortOrder") {
                        preferences.sort_order = sort_order_from_value(map.next_value()?)?;
                    } else if name.eq_ignore_ascii_case("Client") {
                        preferences.client = map.next_value()?;
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(preferences)
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

fn sort_order_from_value<E: de::Error>(value: serde_json::Value) -> Result<SortOrder, E> {
    match value {
        serde_json::Value::String(value) if value.eq_ignore_ascii_case("ascending") => {
            Ok(SortOrder::Ascending)
        }
        serde_json::Value::String(value) if value.eq_ignore_ascii_case("descending") => {
            Ok(SortOrder::Descending)
        }
        serde_json::Value::Number(value) => match value.as_i64() {
            Some(0) => Ok(SortOrder::Ascending),
            Some(1) => Ok(SortOrder::Descending),
            _ => Err(E::custom("invalid sort order")),
        },
        _ => Err(E::custom("invalid sort order")),
    }
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
    let user_id = query
        .user_id
        .as_deref()
        .ok_or_else(|| StatusCode::BAD_REQUEST.into_response())?;
    let client = query
        .client
        .ok_or_else(|| StatusCode::BAD_REQUEST.into_response())?;
    let preferences = state
        .display_preferences_for_request(
            &headers,
            &uri,
            &display_preferences_id,
            Some(user_id),
            query.item_id.as_deref(),
            Some(client),
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
    let user_id = query
        .user_id
        .as_deref()
        .ok_or_else(|| StatusCode::BAD_REQUEST.into_response())?;
    // Emby's generated clients send Client in the DTO for updates.  Retain
    // query precedence for callers that explicitly provide both forms.
    let client = update_client(query.client, &preferences);
    state
        .update_display_preferences_for_request(
            &headers,
            &uri,
            &display_preferences_id,
            Some(user_id),
            query.item_id.as_deref(),
            client,
            display_preferences_request(preferences),
        )
        .await?;
    Ok(StatusCode::OK)
}

fn update_client(
    query_client: Option<String>,
    preferences: &EmbyDisplayPreferences,
) -> Option<String> {
    query_client.or_else(|| preferences.client.as_ref().map(ToOwned::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        http::{Method, Request, header},
    };
    use jellyfin_controller::UserService;
    use jellyfin_data::{DatabaseConfig, DeviceRepository, NewDevice};
    use sea_orm::ConnectionTrait;
    use serde_json::json;
    use tower::ServiceExt;
    use uuid::Uuid;

    const TEST_AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Display Preferences Tests\", DeviceId=\"emby-display-preferences-tests\", Device=\"Test\", Version=\"1.0\"";
    const TEST_DATABASE_PREFIX: &str = "jellyfin_emby_display_preferences_";

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

    #[test]
    fn emby_display_preferences_accept_case_insensitive_wire_names_and_last_value_wins() {
        let preferences: EmbyDisplayPreferences = serde_json::from_str(
            r#"{
                "Id":"discarded",
                "iD":"abc",
                "SortBy":"Name",
                "sOrTbY":"SortName",
                "CustomPrefs":{"theme":"light"},
                "cUsToMpReFs":{"theme":"dark"},
                "SortOrder":"Ascending",
                "sOrToRdEr":"descending",
                "Client":"discarded",
                "cLiEnT":"Emby",
                "Unknown":true
            }"#,
        )
        .unwrap();
        assert_eq!(preferences.id.as_deref(), Some("abc"));
        assert_eq!(preferences.sort_by.as_deref(), Some("SortName"));
        assert_eq!(preferences.sort_order, SortOrder::Descending);
        assert_eq!(preferences.custom_prefs["theme"], "dark");
        assert_eq!(preferences.client.as_deref(), Some("Emby"));

        let numeric: EmbyDisplayPreferences = serde_json::from_value(json!({
            "SortOrder": 1
        }))
        .unwrap();
        assert_eq!(numeric.sort_order, SortOrder::Descending);
    }

    #[test]
    fn emby_display_preferences_query_is_case_insensitive_and_last_value_wins() {
        let query: DisplayPreferencesQuery = serde_json::from_str(
            r#"{
                "UserId":"discarded",
                "uSeRiD":"selected-user",
                "ItemId":"discarded",
                "iTeMiD":"selected-item",
                "Client":"discarded",
                "cLiEnT":"selected-client",
                "Unknown":"ignored"
            }"#,
        )
        .unwrap();
        assert_eq!(query.user_id.as_deref(), Some("selected-user"));
        assert_eq!(query.item_id.as_deref(), Some("selected-item"));
        assert_eq!(query.client.as_deref(), Some("selected-client"));
    }

    #[test]
    fn update_client_prefers_query_and_falls_back_to_body() {
        let preferences = EmbyDisplayPreferences {
            client: Some("body-client".to_owned()),
            ..Default::default()
        };

        assert_eq!(
            update_client(None, &preferences).as_deref(),
            Some("body-client")
        );
        assert_eq!(
            update_client(Some("query-client".to_owned()), &preferences).as_deref(),
            Some("query-client")
        );
        assert_eq!(
            update_client(Some(String::new()), &preferences).as_deref(),
            Some("")
        );
    }

    #[test]
    fn user_settings_array_uses_key_value_entries() {
        let parsed = parse_user_settings(vec!["theme=dark".into(), "empty=".into()]).unwrap();
        assert_eq!(parsed["theme"].as_deref(), Some("dark"));
        assert_eq!(parsed["empty"].as_deref(), Some(""));
        assert!(parse_user_settings(vec!["not-a-setting".into()]).is_err());
        assert_eq!(
            parse_settings_body(br#"{"theme":"dark"}"#).unwrap()["theme"].as_deref(),
            Some("dark")
        );
        assert!(parse_settings_body(br#"{"theme":null}"#).is_err());
    }

    #[tokio::test]
    async fn emby_display_preference_contract_is_protocol_local() {
        let administrator = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        let database_name = format!("{TEST_DATABASE_PREFIX}{}", Uuid::new_v4().simple());
        assert!(
            database_name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        );
        administrator
            .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
            .await
            .expect("temporary PostgreSQL database creation");

        let task_database_name = database_name.clone();
        let outcome = tokio::spawn(async move {
            exercise_display_preference_contract(&task_database_name).await;
        })
        .await;

        administrator
            .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
            .await
            .expect("temporary PostgreSQL database cleanup");
        administrator.close().await.expect("administrator cleanup");
        if let Err(error) = outcome {
            if error.is_panic() {
                std::panic::resume_unwind(error.into_panic());
            }
            panic!("temporary database test task was cancelled: {error}");
        }
    }

    async fn exercise_display_preference_contract(database_name: &str) {
        let mut config = DatabaseConfig::default();
        let (prefix, _) = config
            .url
            .rsplit_once('/')
            .expect("database URL must contain a database name");
        config.url = format!("{prefix}/{database_name}");
        config.max_connections = 8;
        config.min_connections = 1;
        let database = jellyfin_data::connect(&config)
            .await
            .expect("temporary PostgreSQL database");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations");

        let user = UserService::new(database.clone())
            .create_initial_administrator("emby-display-preferences-user")
            .await
            .expect("test user");
        let token = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                user.id,
                "Emby Display Preferences Tests",
                "1.0",
                "Test",
                Uuid::new_v4().simple().to_string(),
            ))
            .await
            .expect("test session")
            .access_token;
        let state = AppState::new(
            database.clone(),
            "Emby Display Preferences Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        let app = jellyfin_api::router(state.clone()).merge(crate::router(state));

        let update_route = format!(
            "/emby/dIsPlAyPrEfErEnCeS/mobile?UserId=not-a-guid&uSeRiD={}",
            user.id
        );
        let response = contract_request(
            &app,
            Method::POST,
            &update_route,
            &token,
            Body::from(
                r#"{
                    "SortBy":"Name",
                    "sOrTbY":"DateCreated",
                    "CustomPrefs":{"theme":"light"},
                    "cUsToMpReFs":{"theme":"dark"},
                    "SortOrder":"Ascending",
                    "sOrToRdEr":"Descending",
                    "Client":"discarded",
                    "cLiEnT":"selected-client"
                }"#,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{update_route}");

        let get_route = format!(
            "/emby/DisplayPreferences/mobile?UserId=not-a-guid&uSeRiD={}&Client=discarded&cLiEnT=selected-client",
            user.id
        );
        let response = contract_request(&app, Method::GET, &get_route, &token, Body::empty()).await;
        assert_eq!(response.status(), StatusCode::OK, "{get_route}");
        let value: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .expect("display preferences response"),
        )
        .expect("display preferences JSON");
        assert_eq!(value["SortBy"], "DateCreated");
        assert_eq!(value["SortOrder"], "Descending");
        assert_eq!(value["CustomPrefs"]["theme"], "dark");
        assert_eq!(value["Client"], "selected-client");

        for route in [
            "/emby/DisplayPreferences/mobile?Client=selected-client",
            "/emby/DisplayPreferences/mobile",
        ] {
            let method = if route.contains("Client=") {
                Method::GET
            } else {
                Method::POST
            };
            let response = contract_request(
                &app,
                method,
                route,
                &token,
                Body::from(r#"{"Client":"selected-client"}"#),
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "generated Emby operation requires UserId: {route}"
            );
        }

        let settings_route = format!("/emby/uSeRsEtTiNgS/{}", user.id);
        let response = contract_request(
            &app,
            Method::POST,
            &settings_route,
            &token,
            Body::from(r#"["theme=dark"]"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{settings_route}");
        let partial_route = format!("{settings_route}/pArTiAl");
        let response = contract_request(
            &app,
            Method::POST,
            &partial_route,
            &token,
            Body::from(r#"["density=compact"]"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{partial_route}");

        for route in [
            "/DisplayPreferences/root-contract?client=web",
            "/api/DisplayPreferences/api-contract?client=web",
        ] {
            let response = contract_request(
                &app,
                Method::POST,
                route,
                &token,
                Body::from(r#"{"SortBy":"DateCreated"}"#),
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::NO_CONTENT,
                "Jellyfin mutation status must remain unchanged: {route}"
            );
        }
        for route in [
            "/DisplayPreferences/root-contract?cLiEnT=web",
            "/api/DisplayPreferences/api-contract?cLiEnT=web",
        ] {
            let response = contract_request(&app, Method::GET, route, &token, Body::empty()).await;
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "Emby query normalization must not change Jellyfin binding: {route}"
            );
        }

        drop(app);
        database.close().await.expect("database cleanup");
    }

    async fn contract_request(
        app: &Router,
        method: Method,
        uri: &str,
        token: &str,
        body: Body,
    ) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(
                        header::AUTHORIZATION,
                        format!("{TEST_AUTHORIZATION}, Token=\"{token}\""),
                    )
                    .body(body)
                    .expect("request"),
            )
            .await
            .expect("response")
    }
}
