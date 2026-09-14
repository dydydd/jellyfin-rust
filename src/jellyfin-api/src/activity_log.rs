use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, State},
    http::HeaderMap,
};
use axum_extra::extract::{Query, QueryRejection};
use jellyfin_data::{
    ActivityLogQuery, ActivityLogSortBy, SortDirection,
    entities::activity_log::{self, LogSeverity},
};
use sea_orm::prelude::DateTimeUtc;
use serde::{Deserialize, Serialize, Serializer};
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ActivityLogParameters {
    #[serde(alias = "StartIndex", alias = "startindex")]
    start_index: Option<i32>,
    #[serde(alias = "Limit")]
    limit: Option<i32>,
    #[serde(alias = "MinDate", alias = "mindate")]
    min_date: Option<DateTimeUtc>,
    #[serde(alias = "MaxDate", alias = "maxdate")]
    max_date: Option<DateTimeUtc>,
    #[serde(alias = "HasUserId", alias = "hasuserid")]
    has_user_id: Option<bool>,
    #[serde(alias = "Name")]
    name: Option<String>,
    #[serde(alias = "Overview")]
    overview: Option<String>,
    #[serde(alias = "ShortOverview", alias = "shortoverview")]
    short_overview: Option<String>,
    #[serde(rename = "type", alias = "Type")]
    activity_type: Option<String>,
    #[serde(alias = "ItemId", alias = "itemid")]
    item_id: Option<Uuid>,
    #[serde(alias = "Username")]
    username: Option<String>,
    #[serde(alias = "Severity")]
    severity: Option<String>,
    #[serde(
        default,
        alias = "SortBy",
        alias = "sortby",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    sort_by: Vec<String>,
    #[serde(
        default,
        alias = "SortOrder",
        alias = "sortorder",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    sort_order: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ActivityLogResult {
    items: Vec<ActivityLogEntry>,
    total_record_count: i32,
    start_index: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct ActivityLogEntry {
    id: i64,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    short_overview: Option<String>,
    #[serde(rename = "Type")]
    activity_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    item_id: Option<String>,
    #[serde(serialize_with = "serialize_date")]
    date: DateTimeUtc,
    user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    severity: Option<&'static str>,
}

pub(crate) async fn entries(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    parameters: Result<Query<ActivityLogParameters>, QueryRejection>,
) -> Result<Json<ActivityLogResult>, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    identity.require_administrator()?;
    let Query(parameters) = parameters.map_err(|_| ApiError::InvalidRequest)?;
    let requested_start_index = parameters.start_index.unwrap_or(0);
    let query = parameters.try_into_query()?;
    let page = state.activity_logs.query(&query).await?;
    let mut items = page
        .items
        .into_iter()
        .map(ActivityLogEntry::from)
        .collect::<Vec<_>>();
    adapt_emby_severities(&uri, &mut items);

    Ok(Json(ActivityLogResult {
        items,
        total_record_count: crate::user_library::checked_int32(page.total_record_count)?,
        start_index: requested_start_index,
    }))
}

impl ActivityLogParameters {
    fn try_into_query(self) -> Result<ActivityLogQuery, ApiError> {
        let sort_by = parse_collection(&self.sort_by, parse_sort_by)?;
        let sort_order = parse_collection(&self.sort_order, parse_sort_direction)?;
        let severity = self
            .severity
            .as_deref()
            .map(|value| parse_severity(value).ok_or(ApiError::InvalidRequest))
            .transpose()?;
        // EF/SQLite preserves the signed controller values in the response,
        // while Skip treats a negative count as zero and SQLite's negative
        // LIMIT is unlimited. Normalize only at the PostgreSQL query boundary.
        let start_index = self.start_index.map(|value| value.max(0) as u64);
        let limit = self.limit.map(|value| {
            if value < 0 {
                i64::MAX as u64
            } else {
                value as u64
            }
        });
        if sort_order.len() > sort_by.len() {
            return Err(ApiError::InvalidRequest);
        }
        let fallback_direction = sort_order
            .first()
            .copied()
            .unwrap_or(SortDirection::Ascending);
        let order_by = sort_by
            .into_iter()
            .enumerate()
            .map(|(index, column)| {
                (
                    column,
                    sort_order.get(index).copied().unwrap_or(fallback_direction),
                )
            })
            .collect();

        Ok(ActivityLogQuery {
            skip: start_index,
            limit,
            min_date: self.min_date,
            max_date: self.max_date,
            has_user_id: self.has_user_id,
            name: self.name,
            overview: self.overview,
            short_overview: self.short_overview,
            activity_type: self.activity_type,
            item_id: self.item_id,
            username: self.username,
            severity,
            order_by,
        })
    }
}

impl From<activity_log::Model> for ActivityLogEntry {
    fn from(entry: activity_log::Model) -> Self {
        Self {
            id: entry.id,
            name: entry.name,
            overview: entry.overview,
            short_overview: entry.short_overview,
            activity_type: entry.activity_type,
            item_id: entry.item_id,
            date: entry.date_created,
            user_id: entry.user_id.simple().to_string(),
            severity: Some(severity_name(entry.log_severity)),
        }
    }
}

fn adapt_emby_severities(uri: &axum::http::Uri, entries: &mut [ActivityLogEntry]) {
    if !uri
        .path()
        .split('/')
        .nth(1)
        .is_some_and(|segment| segment.eq_ignore_ascii_case("emby"))
    {
        return;
    }

    for entry in entries {
        entry.severity = entry.severity.and_then(emby_severity_name);
    }
}

const fn emby_severity_name(severity: &str) -> Option<&'static str> {
    match severity.as_bytes() {
        b"Information" => Some("Info"),
        b"Warning" => Some("Warn"),
        b"Critical" => Some("Fatal"),
        b"Debug" => Some("Debug"),
        b"Error" => Some("Error"),
        // Emby's generated LoggingLogSeverity is closed and has no
        // representation for Jellyfin's Trace or None values. Its Severity
        // property is nullable, so omission is the only lossless adaptation.
        _ => None,
    }
}

fn parse_collection<T>(
    values: &[String],
    parse: impl Fn(&str) -> Option<T>,
) -> Result<Vec<T>, ApiError> {
    values
        .iter()
        .map(|value| parse(value).ok_or(ApiError::InvalidRequest))
        .collect()
}

fn parse_sort_by(value: &str) -> Option<ActivityLogSortBy> {
    match value.to_ascii_lowercase().as_str() {
        "name" | "0" => Some(ActivityLogSortBy::Name),
        "overiew" | "overview" | "1" => Some(ActivityLogSortBy::Overview),
        "shortoverview" | "2" => Some(ActivityLogSortBy::ShortOverview),
        "type" | "3" => Some(ActivityLogSortBy::Type),
        "datecreated" | "5" => Some(ActivityLogSortBy::DateCreated),
        "username" | "6" => Some(ActivityLogSortBy::Username),
        "logseverity" | "7" => Some(ActivityLogSortBy::LogSeverity),
        _ => None,
    }
}

fn parse_sort_direction(value: &str) -> Option<SortDirection> {
    match value.to_ascii_lowercase().as_str() {
        "ascending" | "0" => Some(SortDirection::Ascending),
        "descending" | "1" => Some(SortDirection::Descending),
        _ => None,
    }
}

fn parse_severity(value: &str) -> Option<LogSeverity> {
    match value.to_ascii_lowercase().as_str() {
        "trace" | "0" => Some(LogSeverity::Trace),
        "debug" | "1" => Some(LogSeverity::Debug),
        "information" | "2" => Some(LogSeverity::Information),
        "warning" | "3" => Some(LogSeverity::Warning),
        "error" | "4" => Some(LogSeverity::Error),
        "critical" | "5" => Some(LogSeverity::Critical),
        "none" | "6" => Some(LogSeverity::None),
        _ => None,
    }
}

const fn severity_name(severity: LogSeverity) -> &'static str {
    match severity {
        LogSeverity::Trace => "Trace",
        LogSeverity::Debug => "Debug",
        LogSeverity::Information => "Information",
        LogSeverity::Warning => "Warning",
        LogSeverity::Error => "Error",
        LogSeverity::Critical => "Critical",
        LogSeverity::None => "None",
    }
}

fn serialize_date<S>(value: &DateTimeUtc, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let nanoseconds = value.format("%f").to_string();
    let mut output = format!(
        "{}.{}Z",
        value.format("%Y-%m-%dT%H:%M:%S"),
        &nanoseconds[..7]
    );
    if value.timestamp_subsec_millis() != 0 {
        output.pop();
        while output.ends_with('0') {
            output.pop();
        }
        output.push('Z');
    }
    serializer.serialize_str(&output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_official_sort_and_severity_values() {
        assert_eq!(parse_sort_by("Overiew"), Some(ActivityLogSortBy::Overview));
        assert_eq!(parse_sort_by("7"), Some(ActivityLogSortBy::LogSeverity));
        assert_eq!(
            parse_sort_direction("descending"),
            Some(SortDirection::Descending)
        );
        assert_eq!(parse_severity("Warning"), Some(LogSeverity::Warning));
        assert_eq!(parse_severity("6"), Some(LogSeverity::None));
    }

    #[test]
    fn emby_activity_log_severities_use_the_generated_closed_enum() {
        for (jellyfin, emby) in [
            ("Information", Some("Info")),
            ("Warning", Some("Warn")),
            ("Critical", Some("Fatal")),
            ("Debug", Some("Debug")),
            ("Error", Some("Error")),
            ("Trace", None),
            ("None", None),
        ] {
            assert_eq!(emby_severity_name(jellyfin), emby, "{jellyfin}");
        }
    }

    #[test]
    fn activity_log_severity_adaptation_is_emby_only() {
        fn entries() -> Vec<ActivityLogEntry> {
            vec![ActivityLogEntry {
                id: 1,
                name: "test".to_owned(),
                overview: None,
                short_overview: None,
                activity_type: "test".to_owned(),
                item_id: None,
                date: chrono::Utc::now(),
                user_id: Uuid::nil().simple().to_string(),
                severity: Some("Warning"),
            }]
        }

        let mut emby = entries();
        adapt_emby_severities(
            &"/eMbY/System/ActivityLog/Entries".parse().unwrap(),
            &mut emby,
        );
        assert_eq!(emby[0].severity, Some("Warn"));

        for path in [
            "/System/ActivityLog/Entries",
            "/api/System/ActivityLog/Entries",
        ] {
            let mut jellyfin = entries();
            adapt_emby_severities(&path.parse().unwrap(), &mut jellyfin);
            assert_eq!(jellyfin[0].severity, Some("Warning"), "{path}");
        }
    }
}
