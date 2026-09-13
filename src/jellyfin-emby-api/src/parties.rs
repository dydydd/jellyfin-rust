//! Emby's legacy process-local party service.
//!
//! The 4.10.0.40 server keeps parties, membership, and messages only in the
//! session manager's memory. This protocol-owned registry mirrors that
//! lifetime without adding Party routes or state to Jellyfin's API trees.
//!
//! Its global `JsConfig<PartyInfo>` and `JsConfig<PartyMessage>` converters
//! also replace the nominal Swagger models on the wire: party sessions are
//! `PartySessionInfo { Id, User, IsHost }`, and messages are
//! `PartyMessageDto { DateTime, Message, User }`. The generated Swift fields
//! are nullable and tolerate those runtime-owned fields.

#![allow(clippy::result_large_err)]

use std::{collections::HashMap, fmt, sync::Arc};

use axum::{
    Extension, Json, Router,
    extract::{OriginalUri, Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use jellyfin_api::{AppState, EmbyPartySessionContext, EmbyPartyUser};
use serde::{Deserialize, Deserializer, Serialize, de};
use tokio::sync::Mutex;
use uuid::Uuid;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    let registry = Arc::new(PartyRegistry::default());
    Router::new()
        .route("/Parties", get(list).post(create))
        .route("/parties", get(list).post(create))
        .route("/Parties/Info", get(info))
        .route("/parties/info", get(info))
        .route("/Parties/Messages", get(messages).post(post_message))
        .route("/parties/messages", get(messages).post(post_message))
        .route("/Parties/Leave", axum::routing::post(leave))
        .route("/parties/leave", axum::routing::post(leave))
        .route("/Parties/{party_id}/Join", axum::routing::post(join))
        .route("/parties/{party_id}/join", axum::routing::post(join))
        .layer(Extension(registry))
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
struct PartyInfoResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    party_info: Option<PartyInfoDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
struct PartyInfoDto {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    sessions: Vec<PartySessionDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
struct PartySessionDto {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<EmbyPartyUser>,
    is_host: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
struct PartyMessageDto {
    date_time: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<EmbyPartyUser>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
struct QueryResult<T> {
    items: Vec<T>,
    total_record_count: i32,
}

#[derive(Debug)]
struct PostPartyMessage {
    date_time: Option<DateTime<Utc>>,
    message: Option<String>,
}

impl<'de> Deserialize<'de> for PostPartyMessage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MessageVisitor;

        impl<'de> de::Visitor<'de> for MessageVisitor {
            type Value = PostPartyMessage;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby party message object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut date_time = None;
                let mut message = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key.eq_ignore_ascii_case("UserId") {
                        validate_nullable_i64(map.next_value()?).map_err(de::Error::custom)?;
                    } else if key.eq_ignore_ascii_case("DateTime") {
                        date_time = map.next_value()?;
                    } else if key.eq_ignore_ascii_case("Message") {
                        message = map.next_value()?;
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(PostPartyMessage { date_time, message })
            }
        }

        deserializer.deserialize_map(MessageVisitor)
    }
}

fn validate_nullable_i64(value: serde_json::Value) -> Result<(), &'static str> {
    match value {
        serde_json::Value::Null => Ok(()),
        serde_json::Value::Number(value) => value
            .as_i64()
            .map(|_| ())
            .ok_or("UserId is outside the signed Int64 range"),
        serde_json::Value::String(value) => value
            .parse::<i64>()
            .map(|_| ())
            .map_err(|_| "UserId must be a signed Int64 or numeric string"),
        _ => Err("UserId must be a signed Int64 or numeric string"),
    }
}

#[derive(Default)]
struct PartyRegistry {
    state: Mutex<PartyState>,
}

#[derive(Default)]
struct PartyState {
    parties: Vec<Party>,
    memberships: HashMap<String, String>,
}

struct Party {
    id: String,
    name: Option<String>,
    sessions: Vec<EmbyPartySessionContext>,
    messages: Vec<PartyMessage>,
    master_session_id: Option<String>,
}

struct PartyMessage {
    user: Option<EmbyPartyUser>,
    date_time: DateTime<Utc>,
    message: Option<String>,
}

impl PartyState {
    fn party_index(&self, party_id: &str) -> Option<usize> {
        self.parties
            .iter()
            .position(|party| party.id.eq_ignore_ascii_case(party_id))
    }

    fn party_for_session(&self, session_id: &str) -> Option<&Party> {
        let party_id = self.memberships.get(session_id)?;
        self.party_index(party_id).map(|index| &self.parties[index])
    }

    fn refresh_session(&mut self, session: &EmbyPartySessionContext) {
        let Some(party_id) = self.memberships.get(&session.session_id).cloned() else {
            return;
        };
        let Some(index) = self.party_index(&party_id) else {
            self.memberships.remove(&session.session_id);
            return;
        };
        if let Some(existing) = self.parties[index].sessions.iter_mut().find(|existing| {
            existing
                .session_id
                .eq_ignore_ascii_case(&session.session_id)
        }) {
            *existing = session.clone();
        }
    }

    fn create(
        &mut self,
        session: &EmbyPartySessionContext,
        name: Option<String>,
    ) -> Result<PartyInfoDto, PartyError> {
        let id = Uuid::new_v4().simple().to_string();
        self.parties.push(Party {
            id: id.clone(),
            name,
            sessions: Vec::new(),
            messages: Vec::new(),
            master_session_id: None,
        });
        // Emby inserts the new party before JoinParty rejects a synthetic
        // API-key session which has no user.
        self.join(session, &id)
    }

    fn join(
        &mut self,
        session: &EmbyPartySessionContext,
        party_id: &str,
    ) -> Result<PartyInfoDto, PartyError> {
        let index = self.party_index(party_id).ok_or(PartyError::NotFound)?;
        if session.user.is_none() {
            return Err(PartyError::BadRequest);
        }

        self.memberships
            .insert(session.session_id.clone(), self.parties[index].id.clone());
        let party = &mut self.parties[index];
        if let Some(existing) = party.sessions.iter_mut().find(|existing| {
            existing
                .session_id
                .eq_ignore_ascii_case(&session.session_id)
        }) {
            *existing = session.clone();
        } else {
            party.sessions.push(session.clone());
            if party.sessions.len() == 1 {
                party.master_session_id = Some(session.session_id.clone());
            }
        }
        if party.master_session_id.is_none() && session.has_now_playing_item {
            party.master_session_id = Some(session.session_id.clone());
        }
        Ok(project_party(party))
    }

    fn leave(&mut self, session: &EmbyPartySessionContext) {
        let Some(party_id) = self.memberships.remove(&session.session_id) else {
            return;
        };
        let Some(index) = self.party_index(&party_id) else {
            return;
        };
        let party = &mut self.parties[index];
        let was_host = party
            .master_session_id
            .as_deref()
            .is_some_and(|master| master.eq_ignore_ascii_case(&session.session_id));
        party
            .sessions
            .retain(|member| !member.session_id.eq_ignore_ascii_case(&session.session_id));
        if was_host {
            party.master_session_id = party
                .sessions
                .iter()
                .find(|member| member.has_now_playing_item)
                .map(|member| member.session_id.clone());
        }
        if party.sessions.is_empty() {
            self.parties.remove(index);
        }
    }
}

fn project_party(party: &Party) -> PartyInfoDto {
    PartyInfoDto {
        id: party.id.clone(),
        name: party.name.clone(),
        sessions: party
            .sessions
            .iter()
            .map(|session| PartySessionDto {
                id: session.session_id.clone(),
                user: session.user.clone(),
                is_host: party
                    .master_session_id
                    .as_deref()
                    .is_some_and(|master| master.eq_ignore_ascii_case(&session.session_id)),
            })
            .collect(),
    }
}

#[derive(Debug, Clone, Copy)]
enum PartyError {
    BadRequest,
    NotFound,
}

impl IntoResponse for PartyError {
    fn into_response(self) -> Response {
        match self {
            Self::BadRequest => StatusCode::BAD_REQUEST,
            Self::NotFound => StatusCode::NOT_FOUND,
        }
        .into_response()
    }
}

async fn session(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<EmbyPartySessionContext, Response> {
    state
        .emby_party_session_context_for_request(headers, uri)
        .await
}

async fn list(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Extension(registry): Extension<Arc<PartyRegistry>>,
) -> Result<Json<QueryResult<PartyInfoDto>>, Response> {
    session(&state, &headers, &uri).await?;
    let state = registry.state.lock().await;
    let items = state.parties.iter().map(project_party).collect::<Vec<_>>();
    Ok(Json(QueryResult {
        total_record_count: i32::try_from(items.len()).unwrap_or(i32::MAX),
        items,
    }))
}

async fn create(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Extension(registry): Extension<Arc<PartyRegistry>>,
) -> Result<Json<PartyInfoResult>, Response> {
    let session = session(&state, &headers, &uri).await?;
    let name = query_value(&uri, "Name");
    let party = registry
        .state
        .lock()
        .await
        .create(&session, name)
        .map_err(IntoResponse::into_response)?;
    Ok(Json(PartyInfoResult {
        party_info: Some(party),
    }))
}

async fn join(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Extension(registry): Extension<Arc<PartyRegistry>>,
    Path(party_id): Path<String>,
) -> Result<Json<PartyInfoResult>, Response> {
    let session = session(&state, &headers, &uri).await?;
    let party = registry
        .state
        .lock()
        .await
        .join(&session, &party_id)
        .map_err(IntoResponse::into_response)?;
    Ok(Json(PartyInfoResult {
        party_info: Some(party),
    }))
}

async fn info(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Extension(registry): Extension<Arc<PartyRegistry>>,
) -> Result<Json<PartyInfoResult>, Response> {
    let session = session(&state, &headers, &uri).await?;
    let mut party_state = registry.state.lock().await;
    party_state.refresh_session(&session);
    Ok(Json(PartyInfoResult {
        party_info: party_state
            .party_for_session(&session.session_id)
            .map(project_party),
    }))
}

async fn messages(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Extension(registry): Extension<Arc<PartyRegistry>>,
) -> Result<Json<QueryResult<PartyMessageDto>>, Response> {
    let session = session(&state, &headers, &uri).await?;
    let paging = parse_message_paging(&uri)?;
    let mut party_state = registry.state.lock().await;
    party_state.refresh_session(&session);
    let all = party_state
        .party_for_session(&session.session_id)
        .map_or(&[][..], |party| party.messages.as_slice());
    let total_record_count = i32::try_from(all.len()).unwrap_or(i32::MAX);
    let skip = usize::try_from(paging.start_index.max(0)).unwrap_or(usize::MAX);
    let take = paging
        .limit
        .map_or(usize::MAX, |limit| usize::try_from(limit).unwrap_or(0));
    let items = all
        .iter()
        .skip(skip)
        .take(take)
        .map(|message| PartyMessageDto {
            date_time: message.date_time,
            message: message.message.clone(),
            user: message.user.clone(),
        })
        .collect();
    Ok(Json(QueryResult {
        items,
        total_record_count,
    }))
}

async fn post_message(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Extension(registry): Extension<Arc<PartyRegistry>>,
    request: Result<Json<PostPartyMessage>, JsonRejection>,
) -> Result<StatusCode, Response> {
    let session = session(&state, &headers, &uri).await?;
    let Json(request) = request.map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    // The official handler always overwrites the submitted numeric UserId
    // with the authenticated session's internal user id.
    let PostPartyMessage { date_time, message } = request;
    let mut party_state = registry.state.lock().await;
    party_state.refresh_session(&session);
    let party_id = party_state
        .memberships
        .get(&session.session_id)
        .cloned()
        .ok_or_else(|| PartyError::NotFound.into_response())?;
    let index = party_state
        .party_index(&party_id)
        .ok_or_else(|| PartyError::NotFound.into_response())?;
    party_state.parties[index].messages.push(PartyMessage {
        user: session.user,
        date_time: date_time.unwrap_or_else(Utc::now),
        message,
    });
    Ok(StatusCode::OK)
}

async fn leave(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Extension(registry): Extension<Arc<PartyRegistry>>,
) -> Result<StatusCode, Response> {
    let session = session(&state, &headers, &uri).await?;
    registry.state.lock().await.leave(&session);
    Ok(StatusCode::OK)
}

fn query_value(uri: &Uri, expected: &str) -> Option<String> {
    let mut result = None;
    for (name, value) in form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
        if name.eq_ignore_ascii_case(expected) {
            result = Some(value.into_owned());
        }
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MessagePaging {
    start_index: i32,
    limit: Option<i32>,
}

fn parse_message_paging(uri: &Uri) -> Result<MessagePaging, Response> {
    let mut paging = MessagePaging {
        start_index: 0,
        limit: None,
    };
    for (name, value) in form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
        if name.eq_ignore_ascii_case("StartIndex") {
            paging.start_index = value
                .parse()
                .map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
        } else if name.eq_ignore_ascii_case("Limit") {
            paging.limit = Some(
                value
                    .parse()
                    .map_err(|_| StatusCode::BAD_REQUEST.into_response())?,
            );
        }
    }
    Ok(paging)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_session(id: &str, user: &str, playing: bool) -> EmbyPartySessionContext {
        EmbyPartySessionContext {
            session_id: id.to_owned(),
            user: Some(EmbyPartyUser {
                id: format!("{user}-id"),
                name: user.to_owned(),
            }),
            has_now_playing_item: playing,
        }
    }

    #[test]
    fn registry_matches_create_join_leave_and_host_rules() {
        let mut state = PartyState::default();
        let host = user_session("HOST", "host", false);
        let guest = user_session("guest", "guest", true);
        let created = state.create(&host, Some("Movie Night".to_owned())).unwrap();
        let party_id = created.id;
        assert_eq!(created.sessions.len(), 1);
        assert!(created.sessions[0].is_host);

        let joined = state.join(&guest, &party_id.to_uppercase()).unwrap();
        assert_eq!(joined.sessions.len(), 2);
        assert!(joined.sessions[0].is_host);
        assert!(!joined.sessions[1].is_host);

        state.leave(&host);
        let remaining = state.party_for_session("guest").unwrap();
        assert_eq!(remaining.master_session_id.as_deref(), Some("guest"));
        state.leave(&guest);
        assert!(state.parties.is_empty());
        state.leave(&guest);
    }

    #[test]
    fn api_key_create_preserves_official_empty_party_side_effect() {
        let mut state = PartyState::default();
        let api_key = EmbyPartySessionContext {
            session_id: "api-key:1".to_owned(),
            user: None,
            has_now_playing_item: false,
        };
        assert!(matches!(
            state.create(&api_key, None),
            Err(PartyError::BadRequest)
        ));
        assert_eq!(state.parties.len(), 1);
        assert!(state.parties[0].sessions.is_empty());
    }

    #[test]
    fn hidden_message_paging_is_signed_case_insensitive_and_last_wins() {
        let uri: Uri = "/Parties/Messages?StartIndex=8&startindex=-2&Limit=4&LIMIT=-1"
            .parse()
            .unwrap();
        assert_eq!(
            parse_message_paging(&uri).unwrap(),
            MessagePaging {
                start_index: -2,
                limit: Some(-1),
            }
        );
        assert!(
            parse_message_paging(&"/Parties/Messages?limit=2147483648".parse().unwrap()).is_err()
        );
    }

    #[test]
    fn message_body_matches_json_defaults_and_last_duplicate_wins() {
        let message: PostPartyMessage = serde_json::from_str(
            r#"{"UserId":1,"userid":"2","DateTime":"2026-09-14T01:02:03Z","MESSAGE":"hello","message":"world","Unknown":true}"#,
        )
        .unwrap();
        assert_eq!(message.message.as_deref(), Some("world"));
        assert_eq!(
            message.date_time.unwrap().to_rfc3339(),
            "2026-09-14T01:02:03+00:00"
        );
        assert!(serde_json::from_str::<PostPartyMessage>(r#"{"UserId":"bad"}"#).is_err());
    }
}
