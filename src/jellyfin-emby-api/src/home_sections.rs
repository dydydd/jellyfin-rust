//! Emby user home-section persistence and ordering.
//!
//! Sections are stored in the existing display-preference row for a fixed
//! protocol-owned id/client pair. The encoded JSON array preserves order and
//! keeps these Emby-only DTOs out of Jellyfin's unprefixed response surface.

use std::{fmt, sync::Arc};

use axum::{
    Json, Router,
    extract::{OriginalUri, Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jellyfin_api::AppState;
use jellyfin_model::DisplayPreferencesDto;
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Map, Value};

const DISPLAY_PREFERENCES_ID: &str = "emby-home-sections";
const DISPLAY_PREFERENCES_CLIENT: &str = "Emby.HomeSections";
const CUSTOM_PREFERENCE_KEY: &str = "embyHomeSections";

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Users/{user_id}/HomeSections", get(list).post(upsert))
        .route("/users/{user_id}/homesections", get(list).post(upsert))
        .route("/Users/{user_id}/HomeSections/Delete", post(delete))
        .route("/users/{user_id}/homesections/delete", post(delete))
        .route("/Users/{user_id}/HomeSections/Move", post(move_sections))
        .route("/users/{user_id}/homesections/move", post(move_sections))
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
struct ContentSection(Map<String, Value>);

impl ContentSection {
    fn id(&self) -> Option<&str> {
        self.0.get("Id").and_then(Value::as_str)
    }

    fn ensure_id(&mut self) -> &str {
        if self.id().is_none_or(str::is_empty) {
            self.0.insert(
                "Id".to_owned(),
                Value::String(uuid::Uuid::new_v4().simple().to_string()),
            );
        }
        self.id().expect("a section id was just assigned")
    }
}

impl<'de> Deserialize<'de> for ContentSection {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = case_insensitive_object(deserializer, canonical_content_section_field)?;
        for (name, value) in &mut fields {
            validate_content_section_field::<D::Error>(name, value)?;
        }
        Ok(Self(fields))
    }
}

#[derive(Debug)]
struct DeleteRequest {
    ids: Vec<String>,
}

impl<'de> Deserialize<'de> for DeleteRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = case_insensitive_object(deserializer, |name| {
            name.eq_ignore_ascii_case("Ids").then_some("Ids")
        })?;
        let ids = fields
            .remove("Ids")
            .map(serde_json::from_value::<Option<Vec<String>>>)
            .transpose()
            .map_err(de::Error::custom)?
            .flatten()
            .unwrap_or_default();
        Ok(Self { ids })
    }
}

#[derive(Debug)]
struct MoveRequest {
    ids: Vec<String>,
    new_index: i32,
}

impl<'de> Deserialize<'de> for MoveRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = case_insensitive_object(deserializer, |name| {
            if name.eq_ignore_ascii_case("Ids") {
                Some("Ids")
            } else if name.eq_ignore_ascii_case("NewIndex") {
                Some("NewIndex")
            } else {
                None
            }
        })?;
        let ids = fields
            .remove("Ids")
            .map(serde_json::from_value::<Option<Vec<String>>>)
            .transpose()
            .map_err(de::Error::custom)?
            .flatten()
            .unwrap_or_default();
        let new_index = fields
            .remove("NewIndex")
            .map(serde_json::from_value::<Option<i32>>)
            .transpose()
            .map_err(de::Error::custom)?
            .flatten()
            .unwrap_or_default();
        Ok(Self { ids, new_index })
    }
}

fn case_insensitive_object<'de, D>(
    deserializer: D,
    canonical_field: fn(&str) -> Option<&'static str>,
) -> Result<Map<String, Value>, D::Error>
where
    D: Deserializer<'de>,
{
    struct ObjectVisitor {
        canonical_field: fn(&str) -> Option<&'static str>,
    }

    impl<'de> de::Visitor<'de> for ObjectVisitor {
        type Value = Map<String, Value>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an object")
        }

        fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut normalized = Map::new();
            while let Some(name) = map.next_key::<String>()? {
                let value = map.next_value::<Value>()?;
                if let Some(canonical) = (self.canonical_field)(&name) {
                    // ASP.NET matches properties without regard to case and
                    // retains the final duplicate value.
                    normalized.insert(canonical.to_owned(), value);
                }
            }
            Ok(normalized)
        }
    }

    deserializer.deserialize_map(ObjectVisitor { canonical_field })
}

fn canonical_content_section_field(name: &str) -> Option<&'static str> {
    const FIELDS: &[&str] = &[
        "Name",
        "CustomName",
        "Subtitle",
        "Id",
        "SectionType",
        "CollectionType",
        "ViewType",
        "ImageType",
        "DisplayMode",
        "Monitor",
        "ItemTypes",
        "ExcludedFolders",
        "CardSizeOffset",
        "ScrollDirection",
        "ParentItem",
        "ParentId",
        "TextInfo",
        "PremiumFeature",
        "PremiumMessage",
        "RefreshInterval",
        "SortBy",
        "SortOrder",
        "IncludeNextUpInResume",
        "Query",
    ];
    FIELDS
        .iter()
        .copied()
        .find(|field| field.eq_ignore_ascii_case(name))
}

fn validate_content_section_field<E: de::Error>(name: &str, value: &mut Value) -> Result<(), E> {
    if value.is_null() {
        return Ok(());
    }
    match name {
        "Name" | "CustomName" | "Subtitle" | "Id" | "SectionType" | "CollectionType"
        | "ViewType" | "ImageType" | "DisplayMode" | "ParentId" | "PremiumFeature"
        | "PremiumMessage" | "SortBy" | "SortOrder" => {
            if !value.is_string() {
                return Err(E::custom(format!("{name} must be a string")));
            }
        }
        "Monitor" | "ItemTypes" | "ExcludedFolders" => {
            if !value
                .as_array()
                .is_some_and(|values| values.iter().all(Value::is_string))
            {
                return Err(E::custom(format!("{name} must be a string array")));
            }
        }
        "CardSizeOffset" | "RefreshInterval" => {
            if value
                .as_i64()
                .and_then(|number| i32::try_from(number).ok())
                .is_none()
            {
                return Err(E::custom(format!("{name} must be a signed Int32")));
            }
        }
        "IncludeNextUpInResume" => {
            if !value.is_boolean() {
                return Err(E::custom(format!("{name} must be a boolean")));
            }
        }
        "ParentItem" | "TextInfo" | "Query" => {
            if !value.is_object() {
                return Err(E::custom(format!("{name} must be an object")));
            }
        }
        "ScrollDirection" => {
            let canonical = match value {
                Value::String(name) if name.eq_ignore_ascii_case("Horizontal") || name == "0" => {
                    "Horizontal"
                }
                Value::String(name) if name.eq_ignore_ascii_case("Vertical") || name == "1" => {
                    "Vertical"
                }
                Value::Number(number) if number.as_i64() == Some(0) => "Horizontal",
                Value::Number(number) if number.as_i64() == Some(1) => "Vertical",
                _ => return Err(E::custom("ScrollDirection is invalid")),
            };
            *value = Value::String(canonical.to_owned());
        }
        _ => unreachable!("only canonical ContentSection fields are retained"),
    }
    Ok(())
}

async fn list(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> Result<Json<Vec<ContentSection>>, Response> {
    let (_, sections) = load(&state, &headers, &uri, &user_id).await?;
    Ok(Json(sections))
}

async fn upsert(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    request: Result<Json<ContentSection>, JsonRejection>,
) -> Result<StatusCode, Response> {
    // Resolve and authorize the target before inspecting the body so a caller
    // cannot use malformed JSON to probe another user's preferences.
    let (preferences, mut sections) = load(&state, &headers, &uri, &user_id).await?;
    let Json(section) = request.map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    upsert_section(&mut sections, section);
    save(&state, &headers, &uri, &user_id, preferences, &sections).await
}

async fn delete(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    request: Result<Json<DeleteRequest>, JsonRejection>,
) -> Result<StatusCode, Response> {
    let (preferences, mut sections) = load(&state, &headers, &uri, &user_id).await?;
    let Json(request) = request.map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    delete_sections(&mut sections, &request.ids);
    save(&state, &headers, &uri, &user_id, preferences, &sections).await
}

async fn move_sections(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    request: Result<Json<MoveRequest>, JsonRejection>,
) -> Result<StatusCode, Response> {
    let (preferences, mut sections) = load(&state, &headers, &uri, &user_id).await?;
    let Json(request) = request.map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    move_selected_sections(&mut sections, &request.ids, request.new_index)
        .map_err(IntoResponse::into_response)?;
    save(&state, &headers, &uri, &user_id, preferences, &sections).await
}

async fn load(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
    user_id: &str,
) -> Result<(DisplayPreferencesDto, Vec<ContentSection>), Response> {
    let preferences = state
        .display_preferences_for_request(
            headers,
            uri,
            DISPLAY_PREFERENCES_ID,
            Some(user_id),
            None,
            Some(DISPLAY_PREFERENCES_CLIENT.to_owned()),
        )
        .await?;
    let sections = preferences
        .custom_prefs
        .get(CUSTOM_PREFERENCE_KEY)
        .and_then(Option::as_deref)
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?
        .unwrap_or_default();
    Ok((preferences, sections))
}

async fn save(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
    user_id: &str,
    mut preferences: DisplayPreferencesDto,
    sections: &[ContentSection],
) -> Result<StatusCode, Response> {
    let encoded = serde_json::to_string(sections)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
    preferences
        .custom_prefs
        .insert(CUSTOM_PREFERENCE_KEY.to_owned(), Some(encoded));
    state
        .update_display_preferences_for_request(
            headers,
            uri,
            DISPLAY_PREFERENCES_ID,
            Some(user_id),
            None,
            Some(DISPLAY_PREFERENCES_CLIENT.to_owned()),
            preferences,
        )
        .await?;
    Ok(StatusCode::OK)
}

fn upsert_section(sections: &mut Vec<ContentSection>, mut section: ContentSection) {
    let id = section.ensure_id().to_owned();
    if let Some(index) = sections
        .iter()
        .position(|existing| ids_equal(existing.id(), &id))
    {
        sections[index] = section;
    } else {
        sections.push(section);
    }
}

fn delete_sections(sections: &mut Vec<ContentSection>, ids: &[String]) {
    sections.retain(|section| {
        !ids.iter()
            .any(|requested| ids_equal(section.id(), requested))
    });
}

fn move_selected_sections(
    sections: &mut Vec<ContentSection>,
    ids: &[String],
    new_index: i32,
) -> Result<(), StatusCode> {
    if ids.is_empty() {
        return Ok(());
    }
    let selected_count = sections
        .iter()
        .filter(|section| {
            ids.iter()
                .any(|requested| ids_equal(section.id(), requested))
        })
        .count();
    if selected_count == 0 {
        return Ok(());
    }
    let new_index = usize::try_from(new_index).map_err(|_| StatusCode::BAD_REQUEST)?;
    if new_index > sections.len() - selected_count {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut selected = Vec::with_capacity(selected_count);
    let mut remaining = Vec::with_capacity(sections.len() - selected_count);
    for section in sections.drain(..) {
        if ids
            .iter()
            .any(|requested| ids_equal(section.id(), requested))
        {
            selected.push(section);
        } else {
            remaining.push(section);
        }
    }
    remaining.splice(new_index..new_index, selected);
    *sections = remaining;
    Ok(())
}

fn ids_equal(id: Option<&str>, requested: &str) -> bool {
    id.is_some_and(|id| id.eq_ignore_ascii_case(requested))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use sea_orm::DatabaseConnection;
    use tower::ServiceExt;

    fn section(id: Option<&str>, name: &str) -> ContentSection {
        let mut fields = Map::new();
        if let Some(id) = id {
            fields.insert("Id".to_owned(), Value::String(id.to_owned()));
        }
        fields.insert("Name".to_owned(), Value::String(name.to_owned()));
        ContentSection(fields)
    }

    fn ids(sections: &[ContentSection]) -> Vec<&str> {
        sections.iter().filter_map(ContentSection::id).collect()
    }

    #[test]
    fn content_section_binding_is_case_insensitive_and_sdk_safe() {
        let section: ContentSection = serde_json::from_str(
            r#"{"nAmE":"Latest","ID":"one","scrollDIRECTION":1,"unknown":true}"#,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(section).unwrap(),
            serde_json::json!({
                "Id": "one",
                "Name": "Latest",
                "ScrollDirection": "Vertical"
            })
        );
    }

    #[test]
    fn add_updates_in_place_or_appends_with_an_id() {
        let mut sections = vec![section(Some("one"), "One"), section(Some("two"), "Two")];
        upsert_section(&mut sections, section(Some("ONE"), "Updated"));
        assert_eq!(ids(&sections), ["ONE", "two"]);
        assert_eq!(sections[0].0["Name"], "Updated");

        upsert_section(&mut sections, section(None, "Generated"));
        assert_eq!(sections.len(), 3);
        assert!(!sections[2].id().unwrap().is_empty());
    }

    #[test]
    fn delete_and_move_preserve_stored_relative_order() {
        let mut sections = vec![
            section(Some("one"), "One"),
            section(Some("two"), "Two"),
            section(Some("three"), "Three"),
            section(Some("four"), "Four"),
        ];
        move_selected_sections(&mut sections, &["THREE".to_owned(), "one".to_owned()], 1).unwrap();
        assert_eq!(ids(&sections), ["two", "one", "three", "four"]);

        delete_sections(&mut sections, &["ONE".to_owned(), "four".to_owned()]);
        assert_eq!(ids(&sections), ["two", "three"]);
    }

    #[test]
    fn move_rejects_negative_and_past_end_indexes_without_partial_reorder() {
        for index in [-1, 3] {
            let mut sections = vec![section(Some("one"), "One"), section(Some("two"), "Two")];
            let original = sections.clone();
            assert_eq!(
                move_selected_sections(&mut sections, &["one".to_owned()], index),
                Err(StatusCode::BAD_REQUEST)
            );
            assert_eq!(sections, original);
        }
    }

    #[test]
    fn delete_and_move_bodies_use_last_case_insensitive_duplicate() {
        let delete: DeleteRequest =
            serde_json::from_str(r#"{"Ids":["old"],"iDS":["new"]}"#).unwrap();
        assert_eq!(delete.ids, ["new"]);
        let moved: MoveRequest =
            serde_json::from_str(r#"{"ids":["one"],"newindex":1,"NEWINDEX":0,"ignored":true}"#)
                .unwrap();
        assert_eq!(moved.ids, ["one"]);
        assert_eq!(moved.new_index, 0);
    }

    #[tokio::test]
    async fn every_route_requires_authentication_before_body_binding() {
        let app = crate::router(AppState::new(
            DatabaseConnection::Disconnected,
            "Home Sections Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        let user_id = "00000000-0000-0000-0000-000000000001";
        for (method, route, body) in [
            ("GET", format!("/emby/Users/{user_id}/HomeSections"), ""),
            ("POST", format!("/emby/users/{user_id}/homesections"), "{"),
            (
                "POST",
                format!("/emby/UsErS/{user_id}/HoMeSeCtIoNs/DeLeTe"),
                "{",
            ),
            (
                "POST",
                format!("/emby/uSeRs/{user_id}/hOmEsEcTiOnS/mOvE"),
                "{",
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(&route)
                        .header("content-type", "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{route}");
        }
    }
}
