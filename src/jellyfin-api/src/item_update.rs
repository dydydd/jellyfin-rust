use std::{collections::BTreeMap, fmt, sync::Arc};

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_controller::ItemUpdateInput;
use jellyfin_model::MetadataEditorInfo;
use serde::{Deserialize, Deserializer, de};
use serde_json::Value;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default)]
pub(crate) struct UpdateItemRequest {
    tags: Option<Vec<String>>,
    genres: Option<Vec<String>>,
    studios: Option<Vec<StudioUpdateRequest>>,
    provider_ids: Option<BTreeMap<String, Option<String>>>,
}

impl<'de> Deserialize<'de> for UpdateItemRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RequestVisitor;

        impl<'de> de::Visitor<'de> for RequestVisitor {
            type Value = UpdateItemRequest;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an item metadata object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut request = UpdateItemRequest::default();
                // ASP.NET binds JSON property names case-insensitively, including
                // duplicate names whose last supplied value wins.
                while let Some(key) = map.next_key::<String>()? {
                    match key.to_ascii_lowercase().as_str() {
                        "tags" => request.tags = map.next_value()?,
                        "genres" => request.genres = map.next_value()?,
                        "studios" => request.studios = map.next_value()?,
                        "providerids" => request.provider_ids = map.next_value()?,
                        _ => {
                            map.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(request)
            }
        }

        deserializer.deserialize_map(RequestVisitor)
    }
}

#[derive(Debug, Default)]
struct StudioUpdateRequest {
    name: Option<String>,
}

impl<'de> Deserialize<'de> for StudioUpdateRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StudioVisitor;

        impl<'de> de::Visitor<'de> for StudioVisitor {
            type Value = StudioUpdateRequest;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a studio NameGuidPair object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut studio = StudioUpdateRequest::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.to_ascii_lowercase().as_str() {
                        "name" => {
                            // JsonStringConverter accepts scalar values; null or
                            // an omitted Name remains null in NameGuidPair.
                            studio.name = match map.next_value::<Value>()? {
                                Value::Null => None,
                                Value::String(value) => Some(value),
                                value @ (Value::Bool(_) | Value::Number(_)) => {
                                    Some(value.to_string())
                                }
                                _ => return Err(de::Error::custom("invalid studio name")),
                            };
                        }
                        "id" => {
                            // The action ignores Id, but JsonGuidConverter still
                            // validates it. Null is Guid.Empty; an empty string is invalid.
                            if let Some(id) = map.next_value::<Option<String>>()? {
                                let id = id.trim();
                                let id = if id.starts_with('(') {
                                    id.strip_prefix('(')
                                        .and_then(|id| id.strip_suffix(')'))
                                        .filter(|id| id.len() == 36)
                                        .ok_or_else(|| de::Error::custom("invalid studio id"))?
                                } else {
                                    id
                                };
                                if id.starts_with("urn:") {
                                    return Err(de::Error::custom("invalid studio id"));
                                }
                                Uuid::parse_str(id).map_err(de::Error::custom)?;
                            }
                        }
                        _ => {
                            map.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(studio)
            }
        }

        deserializer.deserialize_map(StudioVisitor)
    }
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UpdateItemContentTypeQuery {
    #[serde(rename = "contentType", alias = "ContentType")]
    content_type: Option<String>,
}

pub(crate) async fn update(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    request: Result<Json<UpdateItemRequest>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    state
        .item_update
        .update(
            item_id,
            ItemUpdateInput {
                tags: request.tags,
                genres: request.genres,
                studios: request
                    .studios
                    .map(|studios| studios.into_iter().map(|studio| studio.name).collect()),
                provider_ids: request.provider_ids,
            },
        )
        .await?;
    crate::websocket::broadcast_library_changed(&state, &[], &[], &[item_id]).await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn update_content_type(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UpdateItemContentTypeQuery>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    state
        .item_update
        .update_content_type(item_id, query.content_type.as_deref())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn metadata_editor(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<Json<MetadataEditorInfo>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(Json(state.metadata_editor.get(item_id).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn studio_name_guid_pairs_follow_official_nullable_and_scalar_binding() {
        let request: UpdateItemRequest = serde_json::from_value(json!({
            "sTuDiOs": [
                {"nAmE": " Studio ", "iD": Uuid::new_v4()},
                {"Name": null, "Id": null},
                {},
                {"name": 42},
                {"NAME": false}
            ],
            "tAgS": ["Tag"],
            "gEnReS": ["Genre"],
            "pRoViDeRiDs": {"Imdb": "tt1234567"}
        }))
        .expect("official case-insensitive scalar binding");
        let names = request
            .studios
            .expect("supplied studios")
            .into_iter()
            .map(|studio| studio.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                Some(" Studio ".to_owned()),
                None,
                None,
                Some("42".to_owned()),
                Some("false".to_owned()),
            ]
        );
        assert_eq!(request.tags, Some(vec!["Tag".to_owned()]));
        assert_eq!(request.genres, Some(vec!["Genre".to_owned()]));
        assert_eq!(
            request.provider_ids.expect("supplied provider ids")["Imdb"].as_deref(),
            Some("tt1234567")
        );
    }

    #[test]
    fn studio_json_preserves_collection_states_and_last_property_value() {
        for body in ["{}", r#"{"Studios":null}"#] {
            assert!(
                serde_json::from_str::<UpdateItemRequest>(body)
                    .expect("nullable studios")
                    .studios
                    .is_none()
            );
        }
        let empty: UpdateItemRequest =
            serde_json::from_str(r#"{"studios":[]}"#).expect("empty studios");
        assert!(empty.studios.expect("explicit studios").is_empty());
        let repeated: UpdateItemRequest = serde_json::from_str(
            r#"{"Studios":[{"Name":"old"}],"STUDIOS":[{"name":"first","NAME":"last"}]}"#,
        )
        .expect("case-insensitive duplicate properties");
        assert_eq!(
            repeated.studios.expect("last studios")[0].name.as_deref(),
            Some("last")
        );
    }

    #[test]
    fn studio_json_validates_pair_shape_and_ignored_guid() {
        let id = Uuid::new_v4();
        for valid in [
            json!(null),
            json!(id.simple().to_string()),
            json!(id.hyphenated().to_string()),
            json!(format!("({id})")),
            json!(format!("{{{id}}}")),
        ] {
            assert!(
                serde_json::from_value::<UpdateItemRequest>(
                    json!({"Studios": [{"Name": "Studio", "Id": valid}]})
                )
                .is_ok()
            );
        }
        for studios in [
            json!("studio"),
            json!(["studio"]),
            json!([{"Name": {"invalid": "object"}}]),
            json!([{"Name": []}]),
            json!([{"Id": ""}]),
            json!([{"Id": "invalid-guid"}]),
            json!([{"Id": 42}]),
            json!([{"Id": format!("urn:uuid:{id}")}]),
        ] {
            assert!(
                serde_json::from_value::<UpdateItemRequest>(json!({"Studios": studios})).is_err()
            );
        }
    }
}
