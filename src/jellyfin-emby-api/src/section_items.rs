//! Emby home-section item queries.
//!
//! Emby stores a small set of query defaults on each `ContentSection`, then
//! exposes the normal user-items query beneath that section.  Keep this route
//! protocol-local and delegate the actual item lookup and DTO projection to
//! Jellyfin's shared, batched `/Users/{userId}/Items` implementation.

use std::{collections::HashSet, convert::Infallible, net::SocketAddr, sync::Arc};

use axum::{
    Router,
    extract::{ConnectInfo, OriginalUri, Path, Request, State},
    http::{HeaderMap, StatusCode, Uri, uri::PathAndQuery},
    response::{IntoResponse, Response},
    routing::get,
};
use jellyfin_api::AppState;
use serde_json::{Map, Value};
use tower::ServiceExt;

const DISPLAY_PREFERENCES_ID: &str = "emby-home-sections";
const DISPLAY_PREFERENCES_CLIENT: &str = "Emby.HomeSections";
const CUSTOM_PREFERENCE_KEY: &str = "embyHomeSections";

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Users/{user_id}/Sections/{section_id}/Items", get(items))
        .route("/users/{user_id}/sections/{section_id}/items", get(items))
}

async fn items(
    State(state): State<Arc<AppState>>,
    OriginalUri(original_uri): OriginalUri,
    headers: HeaderMap,
    Path((user_id, section_id)): Path<(String, String)>,
    mut request: Request,
) -> Result<Response, Response> {
    // Loading the protocol-owned preference first both resolves the target
    // user and preserves the official authorization precedence.  In
    // particular, another user's missing section must not be observable to a
    // non-administrator.
    let preferences = state
        .display_preferences_for_request(
            &headers,
            &original_uri,
            DISPLAY_PREFERENCES_ID,
            Some(&user_id),
            None,
            Some(DISPLAY_PREFERENCES_CLIENT.to_owned()),
        )
        .await?;
    let sections = preferences
        .custom_prefs
        .get(CUSTOM_PREFERENCE_KEY)
        .and_then(Option::as_deref)
        .map(serde_json::from_str::<Value>)
        .transpose()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let sections = sections
        .as_array()
        .ok_or_else(|| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
    let section = sections
        .iter()
        .filter_map(Value::as_object)
        .find(|section| {
            object_field(section, "Id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.eq_ignore_ascii_case(&section_id))
        })
        .ok_or_else(|| StatusCode::NOT_FOUND.into_response())?;

    let query = merged_query(original_uri.query(), section);
    rewrite_as_user_items(request.uri_mut(), &user_id, query.as_deref())
        .map_err(|()| StatusCode::BAD_REQUEST.into_response())?;

    // Axum's matched-path and URL-parameter extensions describe the outer
    // Sections route.  Feeding those private routing extensions into another
    // Router makes its `Path<Uuid>` extractor observe both parameter sets and
    // fail.  Rebuild the internal request with only transport context and the
    // protocol-selecting OriginalUri.
    let connect_info = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .copied();
    let (mut parts, body) = request.into_parts();
    parts.extensions.clear();
    parts.extensions.insert(OriginalUri(original_uri));
    if let Some(connect_info) = connect_info {
        parts.extensions.insert(connect_info);
    }
    let request = Request::from_parts(parts, body);

    // `OriginalUri` remains the original `/emby/...` URI in the request
    // extensions.  The shared handler therefore applies its Emby-only
    // BaseItem adapter while retaining its normal target-user policy,
    // pagination and one-page batched DTO projection.
    Ok(jellyfin_api::unprefixed_router(state.as_ref().clone())
        .oneshot(request)
        .await
        .unwrap_or_else(|error: Infallible| match error {}))
}

fn merged_query(incoming: Option<&str>, section: &Map<String, Value>) -> Option<String> {
    let incoming = incoming.unwrap_or_default();
    let incoming_keys = incoming
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| pair.split_once('=').map_or(pair, |(key, _)| key))
        .map(percent_decode_query_key)
        .map(|key| key.to_ascii_lowercase())
        .collect::<HashSet<_>>();

    let mut pairs = Vec::new();
    push_string_default(&mut pairs, &incoming_keys, section, "ParentId");
    push_string_default(&mut pairs, &incoming_keys, section, "SortBy");
    push_string_default(&mut pairs, &incoming_keys, section, "SortOrder");

    if let Some(query) = object_field(section, "Query").and_then(Value::as_object) {
        for name in ["StudioIds", "TagIds", "GenreIds", "CollectionTypes"] {
            if incoming_keys.contains(&name.to_ascii_lowercase()) {
                continue;
            }
            let Some(values) = object_field(query, name).and_then(Value::as_array) else {
                continue;
            };
            let values = values
                .iter()
                .filter_map(Value::as_str)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            if !values.is_empty() {
                pairs.push((name.to_owned(), values.join(",")));
            }
        }
        for name in [
            "IsFavorite",
            "IsPlayed",
            "IsResumable",
            "IsSports",
            "IsNews",
            "IsSeries",
            "IsMovie",
            "IsRepeat",
        ] {
            if incoming_keys.contains(&name.to_ascii_lowercase()) {
                continue;
            }
            if let Some(value) = object_field(query, name).and_then(Value::as_bool) {
                pairs.push((name.to_owned(), value.to_string()));
            }
        }
    }

    let mut merged = normalize_section_query_keys(incoming);
    for (name, value) in pairs {
        if !merged.is_empty() {
            merged.push('&');
        }
        merged.push_str(&percent_encode_query_component(&name));
        merged.push('=');
        merged.push_str(&percent_encode_query_component(&value));
    }
    (!merged.is_empty()).then_some(merged)
}

fn normalize_section_query_keys(query: &str) -> String {
    query
        .split('&')
        .map(|pair| {
            let (key, suffix) = pair
                .split_once('=')
                .map_or((pair, None), |(key, value)| (key, Some(value)));
            let decoded = percent_decode_query_key(key);
            let key = canonical_section_query_name(&decoded).unwrap_or(key);
            suffix.map_or_else(|| key.to_owned(), |value| format!("{key}={value}"))
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn canonical_section_query_name(name: &str) -> Option<&'static str> {
    const NAMES: &[&str] = &[
        "ParentId",
        "SortBy",
        "SortOrder",
        "StudioIds",
        "TagIds",
        "GenreIds",
        "CollectionTypes",
        "IsFavorite",
        "IsPlayed",
        "IsResumable",
        "IsSports",
        "IsNews",
        "IsSeries",
        "IsMovie",
        "IsRepeat",
    ];
    NAMES
        .iter()
        .copied()
        .find(|candidate| candidate.eq_ignore_ascii_case(name))
}

fn push_string_default(
    pairs: &mut Vec<(String, String)>,
    incoming_keys: &HashSet<String>,
    section: &Map<String, Value>,
    name: &str,
) {
    if incoming_keys.contains(&name.to_ascii_lowercase()) {
        return;
    }
    if let Some(value) = object_field(section, name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        pairs.push((name.to_owned(), value.to_owned()));
    }
}

fn object_field<'a>(object: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    object
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

fn rewrite_as_user_items(uri: &mut Uri, user_id: &str, query: Option<&str>) -> Result<(), ()> {
    let path_and_query = match query {
        Some(query) => format!("/Users/{user_id}/Items?{query}"),
        None => format!("/Users/{user_id}/Items"),
    };
    let path_and_query = PathAndQuery::try_from(path_and_query).map_err(|_| ())?;
    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(path_and_query);
    *uri = Uri::from_parts(parts).map_err(|_| ())?;
    Ok(())
}

fn percent_decode_query_key(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let Some(high) = hex_value(bytes[index + 1]) else {
                    decoded.push(bytes[index]);
                    index += 1;
                    continue;
                };
                let Some(low) = hex_value(bytes[index + 2]) else {
                    decoded.push(bytes[index]);
                    index += 1;
                    continue;
                };
                decoded.push((high << 4) | low);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn percent_encode_query_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(b"0123456789ABCDEF"[usize::from(byte >> 4)]));
            encoded.push(char::from(b"0123456789ABCDEF"[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn section_defaults_preserve_incoming_repeats_and_are_case_insensitively_overridden() {
        let section = json!({
            "ParentId": "parent",
            "SortBy": "SortName",
            "SortOrder": "Descending",
            "Query": {
                "StudioIds": ["one", "two"],
                "IsFavorite": true,
                "IsMovie": true
            }
        });
        let merged = merged_query(
            Some("fields=Overview&FIELDS=ProviderIds&isfavorite=false&sortorder=Ascending"),
            section.as_object().unwrap(),
        )
        .unwrap();
        assert_eq!(
            merged,
            "fields=Overview&FIELDS=ProviderIds&IsFavorite=false&SortOrder=Ascending&ParentId=parent&SortBy=SortName&StudioIds=one%2Ctwo&IsMovie=true"
        );
    }

    #[test]
    fn historical_section_property_casing_and_encoded_query_keys_are_supported() {
        let section = json!({
            "parentid": "stored-parent",
            "query": {"isplayed": true}
        });
        let merged = merged_query(
            Some("%50arent%49d=request-parent"),
            section.as_object().unwrap(),
        )
        .unwrap();
        assert_eq!(merged, "ParentId=request-parent&IsPlayed=true");
    }
}
