use std::{collections::HashMap, sync::Arc};

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, Uri, header},
};
use chrono::{DateTime, Datelike, FixedOffset, Local, Timelike, Utc, Weekday};
use jellyfin_data::{
    NewDevice,
    entities::{api_key, device, user},
};
use jellyfin_model::{DynamicDayOfWeek, UserPolicy};
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{ApiError, AppState, user_to_dto};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct AuthenticateUserByName {
    pub username: Option<String>,
    pub pw: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct QuickConnectDto {
    pub secret: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticationResult {
    user: jellyfin_model::UserDto,
    session_info: Value,
    access_token: String,
    server_id: String,
}

pub(crate) async fn authenticate_by_name(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Result<Json<AuthenticateUserByName>, JsonRejection>,
) -> Result<Json<AuthenticationResult>, ApiError> {
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let username = request.username.unwrap_or_default();
    let password = request.pw.unwrap_or_default();
    let client = ClientMetadata::from_headers(&headers)?;

    let mut user = state
        .users
        .get_by_name(&username)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    if user.is_disabled {
        return Err(ApiError::Forbidden);
    }
    if !user
        .authentication_provider_id
        .eq_ignore_ascii_case(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID)
    {
        return Err(ApiError::Unauthorized);
    }
    let authentication = state.authentication;
    let user = tokio::task::spawn_blocking(move || {
        authentication.authenticate(&username, &password, Some(&mut user))?;
        Ok::<_, jellyfin_server_implementations::AuthenticationError>(user)
    })
    .await
    .map_err(|_| ApiError::Internal)??;
    let policy = stored_user_policy(&user)?;
    if !is_parental_schedule_allowed(&policy, Local::now().fixed_offset()) {
        return Err(ApiError::Forbidden);
    }
    let user = state.users.record_successful_authentication(&user).await?;
    let session = state
        .devices
        .create_session(NewDevice::new(
            user.id,
            &client.client,
            &client.version,
            &client.device,
            &client.device_id,
        ))
        .await?;

    Ok(Json(authentication_result_from_device(
        &state, &user, session,
    )))
}

pub(crate) async fn authenticate_with_quick_connect(
    State(state): State<Arc<AppState>>,
    request: Result<Json<QuickConnectDto>, JsonRejection>,
) -> Result<Json<AuthenticationResult>, ApiError> {
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let secret = request
        .secret
        .filter(|secret| !secret.is_empty())
        .ok_or(ApiError::InvalidRequest)?;
    let authorized = state.quick_connect.get_authorized_request(&secret).await?;
    let session = state
        .devices
        .find_by_token(&authorized.access_token)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    let user = state.users.get(authorized.user_id).await?;
    Ok(Json(authentication_result_from_device(
        &state, &user, session,
    )))
}

fn authentication_result_from_device(
    state: &AppState,
    user: &user::Model,
    session: device::Model,
) -> AuthenticationResult {
    let mut user_dto = user_to_dto(user.clone());
    user_dto.server_id = Some(state.server_id().to_owned());
    AuthenticationResult {
        user: user_dto,
        session_info: json!({
            "Id": session.id.to_string(),
            "UserId": user.id.simple().to_string(),
            "UserName": user.username,
            "Client": session.app_name,
            "ApplicationVersion": session.app_version,
            "DeviceName": session.device_name,
            "DeviceId": session.device_id,
            "IsActive": session.is_active,
        }),
        access_token: session.access_token,
        server_id: state.server_id().to_owned(),
    }
}

pub(crate) async fn current_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<jellyfin_model::UserDto>, ApiError> {
    let authenticated = authenticated_session(&state, &headers).await?;
    let mut dto = user_to_dto(authenticated.user);
    dto.server_id = Some(state.server_id().to_owned());
    Ok(Json(dto))
}

#[derive(Debug)]
pub(crate) struct AuthenticatedSession {
    pub(crate) user: user::Model,
    pub(crate) device: device::Model,
    pub(crate) access_token: String,
    policy: UserPolicy,
}

#[derive(Debug)]
pub(crate) enum AuthenticatedIdentity {
    Device(Box<AuthenticatedSession>),
    ApiKey(api_key::Model),
}

impl AuthenticatedIdentity {
    pub(crate) fn access_token(&self) -> &str {
        match self {
            Self::Device(session) => &session.access_token,
            Self::ApiKey(api_key) => &api_key.access_token,
        }
    }

    pub(crate) fn is_administrator_equivalent(&self) -> bool {
        match self {
            Self::Device(session) => session.user.is_administrator,
            Self::ApiKey(api_key) => !api_key.access_token.is_empty(),
        }
    }

    pub(crate) fn require_administrator(&self) -> Result<(), ApiError> {
        if self.is_administrator_equivalent() {
            Ok(())
        } else {
            Err(ApiError::Forbidden)
        }
    }

    pub(crate) fn require_parental_schedule(
        &self,
        local_now: DateTime<FixedOffset>,
    ) -> Result<(), ApiError> {
        match self {
            Self::Device(session)
                if !session.user.is_administrator
                    && !is_parental_schedule_allowed(&session.policy, local_now) =>
            {
                Err(ApiError::Forbidden)
            }
            Self::Device(_) | Self::ApiKey(_) => Ok(()),
        }
    }

    pub(crate) fn target_user_id(&self, requested: Option<Uuid>) -> Result<Uuid, ApiError> {
        let requested = requested.filter(|user_id| !user_id.is_nil());
        match self {
            Self::Device(session) => match requested {
                Some(user_id) if user_id != session.user.id && !session.user.is_administrator => {
                    Err(ApiError::Forbidden)
                }
                Some(user_id) => Ok(user_id),
                None => Ok(session.user.id),
            },
            Self::ApiKey(_) => Ok(requested.unwrap_or_else(Uuid::nil)),
        }
    }

    pub(crate) fn client_log_file_parts(&self, headers: &HeaderMap) -> (String, String) {
        match self {
            Self::Device(session) => {
                let metadata = headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(parse_authorization)
                    .unwrap_or_default();
                (
                    nonblank(metadata.client).unwrap_or_else(|| session.device.app_name.clone()),
                    nonblank(metadata.version)
                        .unwrap_or_else(|| session.device.app_version.clone()),
                )
            }
            Self::ApiKey(api_key) => (api_key.name.clone(), "apikey".to_owned()),
        }
    }
}

pub(crate) async fn authenticated_session(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<AuthenticatedSession, ApiError> {
    let identity = authenticated_identity(state, headers, None).await?;
    let target_user_id = identity.target_user_id(None)?;
    match identity {
        AuthenticatedIdentity::Device(session) => {
            debug_assert_eq!(target_user_id, session.user.id);
            Ok(*session)
        }
        AuthenticatedIdentity::ApiKey(_) => Err(ApiError::Unauthorized),
    }
}

pub(crate) async fn authenticated_identity(
    state: &AppState,
    headers: &HeaderMap,
    uri: Option<&Uri>,
) -> Result<AuthenticatedIdentity, ApiError> {
    let access_token =
        access_token(headers, uri.and_then(Uri::query)).ok_or(ApiError::Unauthorized)?;

    if let Some(session) = state
        .devices
        .find_by_token(&access_token)
        .await?
        .filter(|session| session.is_active)
    {
        let user = state.users.get(session.user_id).await?;
        if user.is_disabled {
            return Err(ApiError::Forbidden);
        }
        let policy = stored_user_policy(&user)?;
        return Ok(AuthenticatedIdentity::Device(Box::new(
            AuthenticatedSession {
                user,
                device: session.clone(),
                access_token,
                policy,
            },
        )));
    }

    let mut api_key = state
        .api_keys
        .find_by_token(&access_token)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    let touched_at = Utc::now();
    if state.api_keys.touch(&access_token, touched_at).await? != 1 {
        return Err(ApiError::Unauthorized);
    }
    api_key.date_last_activity = touched_at;
    Ok(AuthenticatedIdentity::ApiKey(api_key))
}

pub(crate) fn stored_user_policy(user: &user::Model) -> Result<UserPolicy, ApiError> {
    serde_json::from_value(user.policy.clone()).map_err(|_| ApiError::Internal)
}

fn is_parental_schedule_allowed(policy: &UserPolicy, local_now: DateTime<FixedOffset>) -> bool {
    if policy.access_schedules.is_empty() {
        return true;
    }

    let hour = f64::from(local_now.hour())
        + f64::from(local_now.minute()) / 60.0
        + f64::from(local_now.second()) / 3_600.0
        + f64::from(local_now.nanosecond()) / 3_600_000_000_000.0;
    policy.access_schedules.iter().any(|schedule| {
        schedule_day_matches(schedule.day_of_week, local_now.weekday())
            && hour >= schedule.start_hour
            && hour <= schedule.end_hour
    })
}

const fn schedule_day_matches(day: DynamicDayOfWeek, weekday: Weekday) -> bool {
    match day {
        DynamicDayOfWeek::Everyday => true,
        DynamicDayOfWeek::Weekday => matches!(
            weekday,
            Weekday::Mon | Weekday::Tue | Weekday::Wed | Weekday::Thu | Weekday::Fri
        ),
        DynamicDayOfWeek::Weekend => matches!(weekday, Weekday::Sat | Weekday::Sun),
        DynamicDayOfWeek::Sunday => matches!(weekday, Weekday::Sun),
        DynamicDayOfWeek::Monday => matches!(weekday, Weekday::Mon),
        DynamicDayOfWeek::Tuesday => matches!(weekday, Weekday::Tue),
        DynamicDayOfWeek::Wednesday => matches!(weekday, Weekday::Wed),
        DynamicDayOfWeek::Thursday => matches!(weekday, Weekday::Thu),
        DynamicDayOfWeek::Friday => matches!(weekday, Weekday::Fri),
        DynamicDayOfWeek::Saturday => matches!(weekday, Weekday::Sat),
    }
}

#[derive(Debug, Default)]
struct ClientMetadata {
    client: String,
    device_id: String,
    device: String,
    version: String,
    token: Option<String>,
}

impl ClientMetadata {
    fn from_headers(headers: &HeaderMap) -> Result<Self, ApiError> {
        let header = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .ok_or(ApiError::InvalidRequest)?;
        let metadata = parse_authorization(header);
        if metadata.device_id.is_empty() {
            return Err(ApiError::InvalidRequest);
        }
        Ok(metadata)
    }
}

pub(crate) fn authorization_info_from_headers(
    headers: &HeaderMap,
) -> Result<jellyfin_server_implementations::AuthorizationInfo, ApiError> {
    let metadata = ClientMetadata::from_headers(headers)?;
    Ok(jellyfin_server_implementations::AuthorizationInfo {
        device_name: metadata.device,
        device_id: metadata.device_id,
        app_name: metadata.client,
        app_version: metadata.version,
    })
}

fn access_token(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    if let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| parse_authorization(value).token)
    {
        return Some(token);
    }

    for name in ["x-emby-token", "x-mediabrowser-token"] {
        if let Some(token) = nonempty_header(headers, name) {
            return Some(token);
        }
    }

    let query = query?;
    for name in ["ApiKey", "api_key"] {
        if let Some((_, token)) = form_urlencoded::parse(query.as_bytes())
            .find(|(key, value)| key == name && !value.is_empty())
        {
            return Some(token.into_owned());
        }
    }
    None
}

fn nonempty_header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn nonblank(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn parse_authorization(value: &str) -> ClientMetadata {
    let Some((scheme, fields)) = value.split_once(' ') else {
        return ClientMetadata::default();
    };
    if !scheme.eq_ignore_ascii_case("MediaBrowser") && !scheme.eq_ignore_ascii_case("Emby") {
        return ClientMetadata::default();
    }
    let mut parts = parse_authorization_parts(fields);
    ClientMetadata {
        client: parts.remove("client").unwrap_or_default(),
        device_id: parts.remove("deviceid").unwrap_or_default(),
        device: parts.remove("device").unwrap_or_default(),
        version: parts.remove("version").unwrap_or_default(),
        token: parts.remove("token").filter(|token| !token.is_empty()),
    }
}

fn parse_authorization_parts(value: &str) -> HashMap<String, String> {
    let mut parts = HashMap::new();
    for field in quoted_fields(value) {
        let Some((key, raw_value)) = field.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }

        let value = decode_authorization_value(raw_value);
        // Jellyfin field names are case-insensitive; the last duplicate wins.
        parts.insert(key.to_ascii_lowercase(), value);
    }
    parts
}

fn quoted_fields(value: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;

    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if character == ',' && !quoted {
            fields.push(&value[start..index]);
            start = index + character.len_utf8();
        }
    }

    if !quoted && !escaped {
        fields.push(&value[start..]);
    }
    fields
}

fn decode_authorization_value(value: &str) -> String {
    let value = value.trim();
    let value = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .map_or_else(|| value.to_owned(), unescape_quoted_value);
    percent_decode_str(&value).decode_utf8_lossy().into_owned()
}

fn unescape_quoted_value(value: &str) -> String {
    let mut decoded = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character == '\\'
            && let Some(escaped) = characters.next()
        {
            decoded.push(escaped);
        } else {
            decoded.push(character);
        }
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use chrono::{TimeZone, Timelike};
    use jellyfin_model::AccessSchedule;

    #[test]
    fn parses_official_media_browser_header() {
        let metadata = parse_authorization(
            "MediaBrowser Client=\"Jellyfin.Server%20Integration%20Tests\", \
             DeviceId=\"69420\", Device=\"Apple%20II\", Version=\"10.8.0\", Token=\"abc\"",
        );
        assert_eq!(metadata.client, "Jellyfin.Server Integration Tests");
        assert_eq!(metadata.device_id, "69420");
        assert_eq!(metadata.device, "Apple II");
        assert_eq!(metadata.version, "10.8.0");
        assert_eq!(metadata.token.as_deref(), Some("abc"));
    }

    #[test]
    fn authorization_parts_match_official_matrix() {
        for (input, expected) in [
            (
                "x=\"123,123\",y=\"123\"",
                vec![("x", "123,123"), ("y", "123")],
            ),
            (
                "x=\"123,123\",         y=\"123\",z=\"'hi'\"",
                vec![("x", "123,123"), ("y", "123"), ("z", "'hi'")],
            ),
            ("x=\"ab\"", vec![("x", "ab")]),
            ("param=Hörbücher", vec![("param", "Hörbücher")]),
            ("param=%22%Hörbücher", vec![("param", "\"%Hörbücher")]),
        ] {
            let actual = parse_authorization_parts(input);
            for (key, value) in expected {
                assert_eq!(actual.get(key).map(String::as_str), Some(value));
            }
        }
    }

    #[test]
    fn quoted_commas_and_escaped_quotes_preserve_client_metadata() {
        let metadata = parse_authorization(
            r#"MediaBrowser Client="Jellyfin \"Test\", Inc.", DeviceId="id,one", Device="TV, Main", Version="1.0", Token="abc""#,
        );
        assert_eq!(metadata.client, "Jellyfin \"Test\", Inc.");
        assert_eq!(metadata.device_id, "id,one");
        assert_eq!(metadata.device, "TV, Main");
        assert_eq!(metadata.version, "1.0");
        assert_eq!(metadata.token.as_deref(), Some("abc"));
    }

    #[test]
    fn duplicate_empty_and_malformed_fields_have_stable_rules() {
        let parts = parse_authorization_parts(
            "Client=first,, broken, =ignored, CLIENT=last, Token=abc, token=, literal=100%, incomplete=%2, dangling=\"ignored",
        );
        assert_eq!(parts.get("client").map(String::as_str), Some("last"));
        assert_eq!(parts.get("token").map(String::as_str), Some(""));
        assert_eq!(parts.get("literal").map(String::as_str), Some("100%"));
        assert_eq!(parts.get("incomplete").map(String::as_str), Some("%2"));
        assert!(!parts.contains_key(""));
        assert!(!parts.contains_key("dangling"));

        let metadata = parse_authorization(
            "MediaBrowser Client=first,CLIENT=last,DeviceId=id,Token=abc,token=",
        );
        assert_eq!(metadata.client, "last");
        assert_eq!(metadata.device_id, "id");
        assert_eq!(metadata.token, None);
    }

    #[test]
    fn token_sources_follow_official_precedence() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("MediaBrowser Token=authorization"),
        );
        headers.insert("x-emby-token", HeaderValue::from_static("emby"));
        headers.insert(
            "x-mediabrowser-token",
            HeaderValue::from_static("mediabrowser"),
        );
        assert_eq!(
            access_token(&headers, Some("ApiKey=query-modern&api_key=query-legacy")).as_deref(),
            Some("authorization")
        );

        headers.remove(header::AUTHORIZATION);
        assert_eq!(
            access_token(&headers, Some("ApiKey=query-modern&api_key=query-legacy")).as_deref(),
            Some("emby")
        );
        headers.remove("x-emby-token");
        assert_eq!(
            access_token(&headers, Some("ApiKey=query-modern&api_key=query-legacy")).as_deref(),
            Some("mediabrowser")
        );
        headers.remove("x-mediabrowser-token");
        assert_eq!(
            access_token(&headers, Some("ApiKey=query%20modern&api_key=query-legacy")).as_deref(),
            Some("query modern")
        );
        assert_eq!(
            access_token(&headers, Some("ApiKey=&api_key=query-legacy")).as_deref(),
            Some("query-legacy")
        );

        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer Token=not-a-jellyfin-token"),
        );
        assert_eq!(
            access_token(&headers, Some("ApiKey=query-modern")).as_deref(),
            Some("query-modern")
        );
    }

    #[test]
    fn parental_schedule_day_selectors_and_multiple_ranges_match_official_rules() {
        let monday = local_time(2026, 7, 20, 12, 0, 0);
        let sunday = local_time(2026, 7, 19, 12, 0, 0);
        assert!(is_parental_schedule_allowed(&UserPolicy::default(), monday));

        for (day, monday_allowed, sunday_allowed) in [
            (DynamicDayOfWeek::Monday, true, false),
            (DynamicDayOfWeek::Everyday, true, true),
            (DynamicDayOfWeek::Weekday, true, false),
            (DynamicDayOfWeek::Weekend, false, true),
        ] {
            let policy = policy_with_schedules(vec![schedule(day, 0.0, 24.0)]);
            assert_eq!(
                is_parental_schedule_allowed(&policy, monday),
                monday_allowed
            );
            assert_eq!(
                is_parental_schedule_allowed(&policy, sunday),
                sunday_allowed
            );
        }

        let policy = policy_with_schedules(vec![
            schedule(DynamicDayOfWeek::Sunday, 8.0, 9.0),
            schedule(DynamicDayOfWeek::Monday, 11.0, 13.0),
        ]);
        assert!(is_parental_schedule_allowed(&policy, monday));
    }

    #[test]
    fn parental_schedule_fractional_boundaries_are_inclusive_without_overnight_wrap() {
        let policy = policy_with_schedules(vec![schedule(DynamicDayOfWeek::Everyday, 9.5, 17.25)]);
        assert!(is_parental_schedule_allowed(
            &policy,
            local_time(2026, 7, 20, 9, 30, 0)
        ));
        assert!(is_parental_schedule_allowed(
            &policy,
            local_time(2026, 7, 20, 17, 15, 0)
        ));
        assert!(!is_parental_schedule_allowed(
            &policy,
            local_time(2026, 7, 20, 9, 29, 59)
        ));
        assert!(!is_parental_schedule_allowed(
            &policy,
            local_time(2026, 7, 20, 17, 15, 0)
                .with_nanosecond(1)
                .unwrap()
        ));

        let inverted = policy_with_schedules(vec![schedule(DynamicDayOfWeek::Everyday, 18.0, 6.0)]);
        for hour in [3, 12, 23] {
            assert!(!is_parental_schedule_allowed(
                &inverted,
                local_time(2026, 7, 20, hour, 0, 0)
            ));
        }
    }

    fn local_time(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(8 * 60 * 60)
            .unwrap()
            .with_ymd_and_hms(year, month, day, hour, minute, second)
            .unwrap()
    }

    fn schedule(day_of_week: DynamicDayOfWeek, start_hour: f64, end_hour: f64) -> AccessSchedule {
        AccessSchedule {
            day_of_week,
            start_hour,
            end_hour,
        }
    }

    fn policy_with_schedules(access_schedules: Vec<AccessSchedule>) -> UserPolicy {
        UserPolicy {
            access_schedules,
            ..UserPolicy::default()
        }
    }

    #[test]
    fn api_keys_are_administrator_equivalent_without_an_implicit_user() {
        let identity = AuthenticatedIdentity::ApiKey(api_key::Model {
            id: 1,
            date_created: Utc.timestamp_opt(1, 0).unwrap(),
            date_last_activity: Utc.timestamp_opt(2, 0).unwrap(),
            name: "automation".to_owned(),
            access_token: "key".to_owned(),
        });
        let requested = Uuid::new_v4();

        assert!(identity.is_administrator_equivalent());
        assert_eq!(identity.access_token(), "key");
        assert_eq!(identity.target_user_id(None).unwrap(), Uuid::nil());
        assert_eq!(
            identity.target_user_id(Some(Uuid::nil())).unwrap(),
            Uuid::nil()
        );
        assert_eq!(identity.target_user_id(Some(requested)).unwrap(), requested);
    }
}
