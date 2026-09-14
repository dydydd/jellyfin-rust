//! Emby-only user query contracts missing from Jellyfin's public surface.

mod contracts;

use std::{fmt, sync::Arc};

use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{OriginalUri, Path, Query, Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jellyfin_api::{AppState, EmbyUserCopyOptions};
use jellyfin_model::{NameIdPair, UserDto};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;
use uuid::Uuid;

use contracts::{EmbyUserConfiguration, EmbyUserPolicy};

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Users/Query", get(query_users))
        .route("/users/query", get(query_users))
        .route("/Users/ItemAccess", get(item_access))
        .route("/users/itemaccess", get(item_access))
        .route("/Users/CopyDataOptions", get(copy_data_options))
        .route("/users/copydataoptions", get(copy_data_options))
        .route("/Users/{user_id}/CopyData", post(copy_data))
        .route("/users/{user_id}/copydata", post(copy_data))
        .route("/Users/New", post(create_user))
        .route("/users/new", post(create_user))
        .route("/Users/{user_id}/Password", post(update_password))
        .route("/users/{user_id}/password", post(update_password))
        .route("/Users/Prefixes", get(prefixes))
        .route("/users/prefixes", get(prefixes))
        .route("/Users/{user_id}/Configuration", post(update_configuration))
        .route("/users/{user_id}/configuration", post(update_configuration))
        .route("/Users/{user_id}/Policy", post(update_policy))
        .route("/users/{user_id}/policy", post(update_policy))
}

#[derive(Debug, Default)]
struct UserQuery {
    is_hidden: Option<bool>,
    is_disabled: Option<bool>,
    start_index: Option<i32>,
    limit: Option<i32>,
    name_starts_with_or_greater: Option<String>,
    sort_order: Option<String>,
}

impl<'de> Deserialize<'de> for UserQuery {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = UserQuery;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby user query")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                let mut query = UserQuery::default();
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("IsHidden") {
                        query.is_hidden = Some(map.next_value()?);
                    } else if name.eq_ignore_ascii_case("IsDisabled") {
                        query.is_disabled = Some(map.next_value()?);
                    } else if name.eq_ignore_ascii_case("StartIndex") {
                        query.start_index = Some(map.next_value()?);
                    } else if name.eq_ignore_ascii_case("Limit") {
                        query.limit = Some(map.next_value()?);
                    } else if name.eq_ignore_ascii_case("NameStartsWithOrGreater") {
                        query.name_starts_with_or_greater = Some(map.next_value()?);
                    } else if name.eq_ignore_ascii_case("SortOrder") {
                        query.sort_order = Some(map.next_value()?);
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct UserQueryResult {
    items: Vec<UserDto>,
    total_record_count: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct FullUserCopyDataOptions {
    data_options: Vec<NameIdPair>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct CopyDataRequest {
    /// The generated body repeats the route's UserId property. ServiceStack's
    /// route binding supplies the authoritative value, so this nullable body
    /// value is accepted for SDK compatibility but never overrides the path.
    user_id: Option<String>,
    to_user_ids: Option<Vec<String>>,
    copy_options: Option<Vec<String>>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct CreateUserRequest {
    name: Option<String>,
    copy_from_user_id: Option<String>,
    user_copy_options: Option<Vec<String>>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct UpdateUserPasswordRequest {
    id: Option<String>,
    new_pw: Option<String>,
    reset_password: bool,
}

impl<'de> Deserialize<'de> for UpdateUserPasswordRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = UpdateUserPasswordRequest;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby UpdateUserPassword object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                let mut request = UpdateUserPasswordRequest::default();
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("Id") {
                        request.id = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("NewPw") {
                        request.new_pw = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("ResetPassword") {
                        request.reset_password = map.next_value()?;
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(request)
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

impl<'de> Deserialize<'de> for CreateUserRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = CreateUserRequest;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby CreateUserByName object")
            }
            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                let mut fields = serde_json::Map::new();
                while let Some(name) = map.next_key::<String>()? {
                    let value = map.next_value::<Value>()?;
                    let canonical = ["Name", "CopyFromUserId", "UserCopyOptions"]
                        .iter()
                        .find(|field| field.eq_ignore_ascii_case(&name));
                    if let Some(canonical) = canonical {
                        fields.insert((*canonical).to_owned(), value);
                    }
                }
                Ok(CreateUserRequest {
                    name: fields
                        .remove("Name")
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(de::Error::custom)?
                        .flatten(),
                    copy_from_user_id: fields
                        .remove("CopyFromUserId")
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(de::Error::custom)?
                        .flatten(),
                    user_copy_options: fields
                        .remove("UserCopyOptions")
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(de::Error::custom)?
                        .flatten(),
                })
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}

impl<'de> Deserialize<'de> for CopyDataRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct CopyDataVisitor;

        impl<'de> de::Visitor<'de> for CopyDataVisitor {
            type Value = CopyDataRequest;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby CopyData object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                let mut fields = serde_json::Map::new();
                while let Some(name) = map.next_key::<String>()? {
                    let value = map.next_value::<Value>()?;
                    let canonical = if name.eq_ignore_ascii_case("UserId") {
                        Some("UserId")
                    } else if name.eq_ignore_ascii_case("ToUserIds") {
                        Some("ToUserIds")
                    } else if name.eq_ignore_ascii_case("CopyOptions") {
                        Some("CopyOptions")
                    } else {
                        None
                    };
                    if let Some(canonical) = canonical {
                        // ASP.NET property binding is case-insensitive and the
                        // last duplicate value wins.
                        fields.insert(canonical.to_owned(), value);
                    }
                }
                let mut fields = fields;
                Ok(CopyDataRequest {
                    user_id: fields
                        .remove("UserId")
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(de::Error::custom)?
                        .flatten(),
                    to_user_ids: fields
                        .remove("ToUserIds")
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(de::Error::custom)?
                        .flatten(),
                    copy_options: fields
                        .remove("CopyOptions")
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(de::Error::custom)?
                        .flatten(),
                })
            }
        }

        deserializer.deserialize_map(CopyDataVisitor)
    }
}

async fn update_password(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    request: Result<Json<UpdateUserPasswordRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<StatusCode, Response> {
    let current_token = state
        .emby_password_update_target(&headers, &uri, user_id)
        .await?;
    let Json(request) = request.map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    state
        .persist_emby_password(
            user_id,
            request.new_pw.unwrap_or_default(),
            request.reset_password,
            &current_token,
        )
        .await
}

async fn query_users(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<UserQuery>,
) -> Result<Json<UserQueryResult>, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    Ok(Json(page(
        state.emby_users(query.is_hidden, query.is_disabled).await?,
        query,
    )))
}

async fn item_access(
    State(state): State<Arc<AppState>>,
    Query(query): Query<UserQuery>,
) -> Result<Json<UserQueryResult>, Response> {
    // The protocol middleware has already enforced authentication. ItemAccess
    // intentionally exposes the same user projection as Emby's service.
    Ok(Json(page(
        state.emby_users(query.is_hidden, query.is_disabled).await?,
        query,
    )))
}

async fn update_configuration(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    body: Bytes,
) -> Result<StatusCode, Response> {
    // Authorize and resolve the target before decoding JSON. This preserves
    // Emby's 401/403/404 precedence over a malformed request body.
    let mut shared = state
        .emby_configuration_update_target(&headers, &uri, user_id)
        .await?;
    let configuration: EmbyUserConfiguration =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    configuration.apply_to_shared(&mut shared);
    let wire = serde_json::to_value(configuration)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
    state
        .persist_emby_configuration(user_id, &shared, wire)
        .await
}

async fn update_policy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    body: Bytes,
) -> Result<StatusCode, Response> {
    let (mut shared, current_token) = state
        .emby_policy_update_target(&headers, &uri, user_id)
        .await?;
    let policy: EmbyUserPolicy =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    policy.apply_to_shared(&mut shared);
    let wire = serde_json::to_value(policy)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
    state
        .persist_emby_policy(user_id, &shared, wire, &current_token)
        .await
}

/// Adapts every successful Emby user response emitted by the shared Jellyfin
/// fallback, including nested authentication results. The caller installs
/// this around the Emby route tree only; unprefixed Jellyfin responses never
/// pass through it.
pub(crate) async fn adapt_user_responses(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let should_adapt = is_user_dto_response_path(request.method(), request.uri().path());
    let response = next.run(request).await;
    if !should_adapt || !response.status().is_success() {
        return response;
    }

    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, 8 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    if let Err(response) = adapt_user_json(&state, &mut value).await {
        return response;
    }
    let bytes = match serde_json::to_vec(&value) {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(bytes))
}

fn is_user_dto_response_path(method: &Method, path: &str) -> bool {
    let segments = path
        .strip_prefix("/emby/")
        .unwrap_or(path)
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let Some(first) = segments.first() else {
        return false;
    };
    if !first.eq_ignore_ascii_case("Users") {
        return false;
    }
    match (method, segments.as_slice()) {
        (&Method::GET, [_]) => true,
        (&Method::GET, [_, action]) => {
            action.parse::<Uuid>().is_ok()
                || ["Public", "Me", "Query", "ItemAccess"]
                    .iter()
                    .any(|candidate| action.eq_ignore_ascii_case(candidate))
        }
        (&Method::POST, [_]) => true,
        (&Method::POST, [_, action]) => {
            action.parse::<Uuid>().is_ok()
                || ["New", "AuthenticateByName"]
                    .iter()
                    .any(|candidate| action.eq_ignore_ascii_case(candidate))
        }
        (&Method::POST, [_, _user_id, action]) => action.eq_ignore_ascii_case("Authenticate"),
        _ => false,
    }
}

async fn adapt_user_json(state: &AppState, value: &mut Value) -> Result<(), Response> {
    let mut ids = Vec::new();
    collect_user_ids(value, &mut ids);
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(());
    }
    let stored = state.emby_user_contract_storage(&ids).await?;
    replace_user_contracts(value, &stored);
    Ok(())
}

fn collect_user_ids(value: &Value, ids: &mut Vec<Uuid>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_user_ids(value, ids);
            }
        }
        Value::Object(object) => {
            if object.contains_key("Configuration")
                && object.contains_key("Policy")
                && let Some(id) = object
                    .get("Id")
                    .and_then(Value::as_str)
                    .and_then(|id| id.parse().ok())
            {
                ids.push(id);
            }
            for value in object.values() {
                collect_user_ids(value, ids);
            }
        }
        _ => {}
    }
}

fn replace_user_contracts(
    value: &mut Value,
    stored: &std::collections::HashMap<Uuid, (Value, Value, bool)>,
) {
    match value {
        Value::Array(values) => {
            for value in values {
                replace_user_contracts(value, stored);
            }
        }
        Value::Object(object) => {
            let id = object
                .get("Id")
                .and_then(Value::as_str)
                .and_then(|id| id.parse().ok());
            if object.contains_key("Configuration")
                && object.contains_key("Policy")
                && let Some((preferences, policy, enable_local_password)) =
                    id.and_then(|id| stored.get(&id))
            {
                let configuration =
                    EmbyUserConfiguration::from_storage(preferences, *enable_local_password);
                let policy = EmbyUserPolicy::from_storage(policy);
                if let Ok(configuration) = serde_json::to_value(configuration) {
                    object.insert("Configuration".to_owned(), configuration);
                }
                if let Ok(policy) = serde_json::to_value(policy) {
                    object.insert("Policy".to_owned(), policy);
                }
            }
            for value in object.values_mut() {
                replace_user_contracts(value, stored);
            }
        }
        _ => {}
    }
}

async fn copy_data_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<FullUserCopyDataOptions>, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    Ok(Json(FullUserCopyDataOptions {
        // This order and these ids are used by Emby's 4.10 dashboard
        // `users/usernew.js` when constructing UserCopyOptions.
        data_options: vec![
            NameIdPair {
                name: "User Policy".to_owned(),
                id: "UserPolicy".to_owned(),
            },
            NameIdPair {
                name: "User Configuration".to_owned(),
                id: "UserConfiguration".to_owned(),
            },
            NameIdPair {
                name: "User Data".to_owned(),
                id: "UserData".to_owned(),
            },
        ],
    }))
}

async fn create_user(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<UserDto>, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    let request: CreateUserRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    let name = request
        .name
        .as_deref()
        .ok_or_else(|| StatusCode::BAD_REQUEST.into_response())?;
    let source_user_id = request
        .copy_from_user_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| Uuid::parse_str(value).map_err(|_| StatusCode::BAD_REQUEST.into_response()))
        .transpose()?;
    // Emby's dashboard always sends its checkbox array explicitly. The
    // generated API contract makes this field nullable, so an omitted or
    // explicitly empty array conservatively copies no category.
    let options = if source_user_id.is_some() {
        match request.user_copy_options.as_deref() {
            Some(values) => parse_copy_options(values)?,
            None => EmbyUserCopyOptions::default(),
        }
    } else {
        EmbyUserCopyOptions::default()
    };
    state
        .create_emby_user_with_copy(name, source_user_id, options)
        .await
        .map(Json)
}

async fn copy_data(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(source_user_id): Path<String>,
    body: Bytes,
) -> Result<StatusCode, Response> {
    let (source_user_id, current_token) = state
        .resolve_emby_copy_data_source(&headers, &uri, &source_user_id)
        .await?;
    let request: CopyDataRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    let CopyDataRequest {
        user_id: _,
        to_user_ids,
        copy_options,
    } = request;
    let target_user_ids = to_user_ids
        .filter(|targets| !targets.is_empty())
        .ok_or_else(|| StatusCode::BAD_REQUEST.into_response())?
        .into_iter()
        .map(|target| Uuid::parse_str(&target).map_err(|_| StatusCode::BAD_REQUEST.into_response()))
        .collect::<Result<Vec<_>, _>>()?;
    let copy_options = copy_options.unwrap_or_default();
    let options = parse_copy_options(&copy_options)?;
    state
        .copy_emby_user_state(source_user_id, &target_user_ids, options, &current_token)
        .await?;
    Ok(StatusCode::OK)
}

fn parse_copy_options(values: &[String]) -> Result<EmbyUserCopyOptions, Response> {
    let mut options = EmbyUserCopyOptions::default();
    for value in values {
        if value.eq_ignore_ascii_case("UserPolicy") {
            options.policy = true;
        } else if value.eq_ignore_ascii_case("UserConfiguration") {
            options.configuration = true;
        } else if value.eq_ignore_ascii_case("UserData") {
            options.user_data = true;
        } else {
            return Err(StatusCode::BAD_REQUEST.into_response());
        }
    }
    Ok(options)
}

async fn prefixes(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<UserQuery>,
) -> Result<Json<Vec<NameIdPair>>, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    let users = state.emby_users(query.is_hidden, query.is_disabled).await?;
    let mut prefixes = Vec::new();
    for user in users {
        let Some(name) = user.name else { continue };
        if query
            .name_starts_with_or_greater
            .as_deref()
            .is_some_and(|start| name.as_str() < start)
        {
            continue;
        }
        let Some(prefix) = name.chars().next().map(|c| c.to_uppercase().to_string()) else {
            continue;
        };
        if !prefixes.iter().any(|item: &NameIdPair| item.name == prefix) {
            prefixes.push(NameIdPair {
                name: prefix.clone(),
                id: prefix,
            });
        }
    }
    prefixes.sort_by(|a, b| a.name.cmp(&b.name));
    if query
        .sort_order
        .as_deref()
        .is_some_and(|order| order.eq_ignore_ascii_case("descending") || order == "1")
    {
        prefixes.reverse();
    }
    let start = query.start_index.unwrap_or(0);
    let offset = start.max(0) as usize;
    let end = match query.limit {
        Some(0) => offset,
        Some(limit) if limit > 0 => offset.saturating_add(limit as usize),
        _ => prefixes.len(),
    }
    .min(prefixes.len());
    let prefixes = if offset < prefixes.len() {
        prefixes[offset..end].to_vec()
    } else {
        Vec::new()
    };
    Ok(Json(prefixes))
}

fn page(mut users: Vec<UserDto>, query: UserQuery) -> UserQueryResult {
    if let Some(prefix) = query.name_starts_with_or_greater.as_deref() {
        users.retain(|user| user.name.as_deref().is_some_and(|name| name >= prefix));
    }
    if query
        .sort_order
        .as_deref()
        .is_some_and(|order| order.eq_ignore_ascii_case("descending") || order == "1")
    {
        users.reverse();
    }
    let total = users.len().min(i32::MAX as usize) as i32;
    let start = query.start_index.unwrap_or(0);
    let offset = start.max(0) as usize;
    let limit = query.limit.unwrap_or(-1);
    let items = if limit == 0 || offset >= users.len() {
        Vec::new()
    } else {
        let end = if limit < 0 {
            users.len()
        } else {
            offset.saturating_add(limit as usize).min(users.len())
        };
        users[offset..end].to_vec()
    };
    UserQueryResult {
        items,
        total_record_count: total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use sea_orm::DatabaseConnection;
    use tower::ServiceExt;

    #[test]
    fn signed_paging_matches_emby_contract() {
        let users = (0..3)
            .map(|n| UserDto {
                name: Some(format!("{n}")),
                ..Default::default()
            })
            .collect();
        let result = page(
            users,
            UserQuery {
                start_index: Some(-2),
                limit: Some(1),
                ..Default::default()
            },
        );
        assert_eq!(result.total_record_count, 3);
        assert_eq!(result.items.len(), 1);
        let wire = serde_json::to_value(result).expect("user query response");
        let wire = wire.as_object().expect("user query object");
        assert_eq!(wire.len(), 2);
        assert!(wire.contains_key("Items"));
        assert_eq!(wire["TotalRecordCount"], 3);
        assert!(!wire.contains_key("StartIndex"));
    }

    #[test]
    fn user_query_names_are_case_insensitive_and_last_duplicate_wins() {
        let query: UserQuery = serde_json::from_str(
            r#"{
                "sTaRtInDeX": -2,
                "LiMiT": 1,
                "LIMIT": 2,
                "iShIdDeN": true,
                "IsDiSaBlEd": false,
                "nAmEsTaRtSwItHoRgReAtEr": "M",
                "sOrToRdEr": "Descending",
                "ignored": {"nested": true}
            }"#,
        )
        .expect("case-insensitive user query");
        assert_eq!(query.start_index, Some(-2));
        assert_eq!(query.limit, Some(2));
        assert_eq!(query.is_hidden, Some(true));
        assert_eq!(query.is_disabled, Some(false));
        assert_eq!(query.name_starts_with_or_greater.as_deref(), Some("M"));
        assert_eq!(query.sort_order.as_deref(), Some("Descending"));
    }

    #[test]
    fn copy_data_options_keep_swift_decodable_shape() {
        let value = serde_json::to_value(FullUserCopyDataOptions {
            data_options: vec![
                NameIdPair {
                    name: "User Policy".to_owned(),
                    id: "UserPolicy".to_owned(),
                },
                NameIdPair {
                    name: "User Configuration".to_owned(),
                    id: "UserConfiguration".to_owned(),
                },
                NameIdPair {
                    name: "User Data".to_owned(),
                    id: "UserData".to_owned(),
                },
            ],
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "DataOptions": [
                    {"Name": "User Policy", "Id": "UserPolicy"},
                    {"Name": "User Configuration", "Id": "UserConfiguration"},
                    {"Name": "User Data", "Id": "UserData"}
                ]
            })
        );
    }

    #[test]
    fn copy_data_body_is_case_insensitive_and_last_duplicate_wins() {
        let request: CopyDataRequest = serde_json::from_str(
            r#"{
                "USERID":"ignored",
                "touserids":["old"],
                "ToUserIds":["new"],
                "copyOPTIONS":["UserData"]
            }"#,
        )
        .expect("copy data request");
        assert_eq!(request.user_id.as_deref(), Some("ignored"));
        assert_eq!(request.to_user_ids, Some(vec!["new".to_owned()]));
        assert_eq!(request.copy_options, Some(vec!["UserData".to_owned()]));
    }

    #[test]
    fn password_body_matches_generated_fields_case_insensitively_and_last_wins() {
        let request: UpdateUserPasswordRequest = serde_json::from_str(
            r#"{
                "Id":"body-id",
                "NewPw":"old",
                "nEwPw":"new",
                "ResetPassword":true,
                "rEsEtPaSsWoRd":false,
                "Ignored":"value"
            }"#,
        )
        .expect("Emby password request");
        assert_eq!(
            request,
            UpdateUserPasswordRequest {
                id: Some("body-id".to_owned()),
                new_pw: Some("new".to_owned()),
                reset_password: false,
            }
        );
    }

    #[test]
    fn response_adapter_is_limited_to_user_dto_routes() {
        let user_id = Uuid::new_v4();
        for (method, path) in [
            (&Method::GET, "/Users".to_owned()),
            (&Method::GET, "/Users/Public".to_owned()),
            (&Method::GET, "/Users/Me".to_owned()),
            (&Method::GET, "/Users/Query".to_owned()),
            (&Method::GET, "/Users/ItemAccess".to_owned()),
            (&Method::GET, format!("/Users/{user_id}")),
            (&Method::POST, "/Users/New".to_owned()),
            (&Method::POST, "/Users/AuthenticateByName".to_owned()),
            (&Method::POST, format!("/Users/{user_id}/Authenticate")),
            (&Method::POST, format!("/emby/Users/{user_id}")),
        ] {
            assert!(is_user_dto_response_path(method, &path), "{path}");
        }

        for (method, path) in [
            (&Method::GET, format!("/Users/{user_id}/Items")),
            (&Method::GET, format!("/Users/{user_id}/Views")),
            (&Method::GET, "/Users/Prefixes".to_owned()),
            (&Method::POST, format!("/Users/{user_id}/Configuration")),
            (&Method::POST, format!("/Users/{user_id}/Policy")),
            (
                &Method::POST,
                "/Users/AuthenticateWithQuickConnect".to_owned(),
            ),
        ] {
            assert!(!is_user_dto_response_path(method, &path), "{path}");
        }
    }

    #[tokio::test]
    async fn copy_data_options_routes_require_administrator() {
        let app = routes().with_state(Arc::new(AppState::new(
            DatabaseConnection::Disconnected,
            "test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )));
        for path in ["/Users/CopyDataOptions", "/users/copydataoptions"] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        }
    }
}
